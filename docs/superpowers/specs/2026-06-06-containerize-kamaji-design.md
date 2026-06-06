# Design — containerized kamaji + the `kamaji up`/`down` launcher

## Problem

kamaji spawns AI agents (Claude Code, Codex, Copilot) as zellij sessions that
run with **the same privileges as the user who launched the daemon**. An agent
in `--dangerously-skip-permissions` / "YOLO" mode can therefore touch anything
the user can: the whole home directory, every project, system packages, the
network. People who want to let an agent run unattended need a blast-radius
boundary, and "just run it on a throwaway machine" is too heavy for everyday
use.

A container is the natural boundary. The question this spec answers is **not**
"is it possible" (it plainly is) but "what is the *user experience* of shipping,
installing, launching, and shutting kamaji down as a containerized app, such
that an agent can have full root inside the box without risking the host."

kamaji is unusually well-suited to this because it is already **browser-first**:
agent terminals are streamed into the board via `zellij web`, not the user's
local TTY. So the container can hold the daemon *and* zellij *and* every agent,
and the user drives the whole thing from `http://localhost:8755` in their host
browser — no host terminal access to the agents is required.

## Goal

A first-class containerized mode for kamaji, delivered as one cohesive change:

1. A published container image holding the daemon, zellij, git, and the agent
   CLIs.
2. A `kamaji up` / `kamaji down` launcher (new subcommands on the existing
   `kamaji` binary) that starts/stops that container with all the fiddly
   bind/port/mount/credential/limit wiring derived automatically.
3. Documented "raw" escape hatches (a Podman Quadlet unit and a Compose file)
   for users who prefer to manage the container themselves.

Success: a user runs `curl install.sh | sh` (unchanged) → `kamaji up` → opens
`localhost:8755` → works agents that have root **inside the container only** →
`kamaji down`. Board state and resumable sessions survive `down`/`up`.

## Trust model

**One container holds daemon + zellij + every agent. That single boundary is the
sandbox.** An agent's reachable surface is exactly: the container's own
filesystem, plus the project roots the user mounted. Everything else on the host
— the rest of `$HOME`, other projects, system packages, host devices, the host
network beyond published ports — is out of reach.

The boundary is honest about what it does *not* protect: a mounted project root
is read-write (agents must edit code), so an agent can still damage files in the
projects it was handed, and — because git worktrees share the repo's object
store — could corrupt that one repository. That residual risk is inherent to
"let the agent work on my code" and is the accepted scope of the sandbox.

## Decisions

These were proposed during brainstorming and approved.

1. **Rootless Podman is the first-class runtime; Docker is supported.** Under
   rootless Podman, container-UID-0 maps to the *unprivileged* host user via a
   user namespace. So "the agent is root in the container" means "the agent has,
   at most, your normal host-user rights, confined to the container's mounts" —
   which is exactly the property the whole feature exists to provide. Plain
   Docker maps container-root to host-root, so a container escape is materially
   worse; we recommend rootless Docker or `--userns-remap` and document the gap.

2. **The in-container user is root** (mapped safely per Decision 1), so agents
   can `sudo`, install packages, and otherwise do anything *inside* the box.
   Under rootless Podman's default mapping, files an agent writes into a mounted
   project come back owned by the host user, so there is no root-owned-files
   papercut on the host.

3. **Credentials are mounted by default**, with env passthrough as an
   alternative: the launcher bind-mounts the agent credential directories
   (`~/.claude`, `~/.codex`, `~/.config/...`) and passes through agent API-key
   env vars (`ANTHROPIC_API_KEY`, etc.) when set.

4. **Resource limits are on by default and overridable.** The launcher sets
   `--memory`, `--pids-limit`, and `--cpus` to sane defaults so a runaway agent
   can't DoS the host; flags override them.

5. **Code is mounted, not cloned.** `kamaji up` reads the user's registered
   projects from the database and bind-mounts each project root **read-write at
   an identical in-container path**. Agents work on the real local checkouts
   (including uncommitted changes), host editors/git stay in sync, and only
   registered projects are exposed. Mounting at the identical path is also what
   keeps git worktrees valid (see Architecture → worktree consistency).

## Architecture

Three deliverables, specified together.

### Deliverable A — the image (`ghcr.io/alveflo/kamaji`)

A debian-slim base (not musl/static: the Node-based agent CLIs need a full glibc
userland) containing:

- `kamajid` (release build) — the entrypoint is `kamajid serve`.
- `zellij` ≥ 0.43 (provides `zellij web`, which the daemon manages).
- `git`.
- Node.js plus the agent CLIs: `claude`, `codex`, `copilot`.
- `ca-certificates`.

These are exactly the binaries the daemon shells out to: agent argv comes from
`agent.rs` / config templates, and zellij/git are invoked from `zellij.rs` /
`git.rs`. The image is built and published by CI; the build smoke-tests
`kamajid --version`, `zellij --version`, and the presence of each agent CLI.

Image tags are pinned to the kamaji version so the client and the containerized
daemon never skew (the daemon already exposes its version via `/healthz`, which
the client uses to warn on mismatch).

### Deliverable B — container run conventions

What `kamaji up` encodes (and what the Quadlet/Compose escape hatches document).

**Networking.** `zellij web` is hardcoded to `127.0.0.1:8082`
(`zellij_web.rs::DEFAULT_BASE_URL`) and the daemon binds `127.0.0.1:8755` by
default. A process bound to loopback *inside* a container is unreachable from the
host even with port publishing, so:

- The daemon binds `0.0.0.0:8755`. `derive_proxy_addr()` (in `kamajid/main.rs`)
  then derives the proxy bind `0.0.0.0:8756` and — already implemented —
  rewrites the iframe's *public* host back to `127.0.0.1`, which is correct when
  the browser runs on the same host as the container.
- Publish **`127.0.0.1:8755:8755`** and **`127.0.0.1:8756:8756`** — bound to the
  host's loopback so the board is never exposed to the LAN.
- Port **8082 stays internal**: the proxy reaches `zellij web` over loopback
  *inside* the container, and the browser only ever hits the proxy on 8756.

**Worktree-path consistency (the load-bearing detail).** `git worktree add`
(see `git.rs`) writes absolute-path links *both ways*: the worktree's `.git`
file points at `<repo>/.git/worktrees/<name>`, and that directory's `gitdir`
file points back at the worktree's absolute path. Those paths must resolve
identically wherever git runs. Decision 5 (mount each project root at its
**identical** path) guarantees daemon-path == host-path == worktree-path, so
worktrees stay valid both inside the container and to host-side tooling.
`worktree_base` is set to a path *under* a mounted root so the worktrees land on
a real mount.

`worktree_base` has **no default** in config (`config.rs`): a fresh config
leaves it unset and the TUI normally prompts. Headless/browser-only there is no
prompt, so the launcher must set `worktree_base` explicitly in the generated
config (below).

**State & persistence.**

- The kamaji **data dir** (`~/.local/share/kamaji`, holding `kamaji.db`) and
  **config dir** (`~/.config/kamaji`) are bind-mounted at identical host paths.
  Sharing these files is how the host launcher reads the project list to derive
  mounts, and how `kamaji up` writes the generated config the daemon then loads.
- zellij's **cache** (its session-resurrection state) is a named volume, so
  exited sessions stay resurrectable across `down`/`up`.
- Project roots are bind-mounted per Decision 5.
- Credentials are mounted per Decision 3.

**Generated config.** `kamaji up` writes a container-flavored `config.toml` into
the shared config dir, merged from the user's existing settings with these
overrides: `daemon.bind = "0.0.0.0:8755"` and a concrete `worktree_base`. All
other settings (agents, theme, auto-review) carry through unchanged.

**Identity & limits.** The container runs as root-in-container (Decision 2) under
rootless-Podman userns mapping (Decision 1), with the default resource limits of
Decision 4.

### Deliverable C — the launcher (`kamaji up` / `kamaji down` / `kamaji logs`)

New subcommands on the existing `kamaji` binary. This mirrors what `kamaji`
already does — it auto-spawns a *local* `kamajid` today; here it spawns a
*containerized* one. Same philosophy, bigger box.

`kamaji up`:

1. **Detect the runtime.** Prefer rootless `podman`, else `docker`; `--runtime`
   overrides. If neither is found, print an install hint and exit.
2. **Derive project mounts.** Read the shared `kamaji.db` and emit a
   `-v <root>:<root>` for each registered project root.
3. **Generate config** into the shared config dir (bind + worktree_base, as
   above).
4. **Assemble the run command:** published ports, the data/config/zellij-cache
   volumes, project mounts, credential mounts, env passthrough, resource limits,
   userns mapping, image tag, and pull policy.
5. **Run detached**, then poll `/healthz` on the published board port until ready
   and print the board URL.
6. **Record container state** (container name + published board address) in a
   small file in the runtime dir, so the client knows a container daemon owns
   this kamaji (see "How existing kamaji adapts").

`kamaji down` stops and removes the container; the named/bind volumes persist, so
no board state or resumable sessions are lost. `kamaji logs` tails the
container's logs (for surfacing agent/zellij/daemon errors).

## How existing kamaji adapts

- **Client connection.** `kamaji ticket create` and the TUI keep working — they
  talk to the published `127.0.0.1:8755`. The discovery path (`daemon.rs
  ensure_daemon()`) gains a container-aware first step: if the container-state
  file recorded by `kamaji up` exists and its `/healthz` is good, connect to it
  and **do not** auto-spawn a local daemon; if it exists but is unhealthy, tell
  the user to run `kamaji up` rather than silently starting a conflicting local
  daemon. With no container-state file, behavior is exactly as today.

- **TUI attach in container mode (Decision recorded in brainstorming).** The
  TUI's "attach" shells out to `zellij attach`, which speaks to the zellij server
  *inside* the container over a socket the host cannot see. In container mode,
  the attach path instead runs `<runtime> exec -it <container> zellij attach
  <name>`. The browser remains the primary surface; this is the TUI convenience
  path.

## Known limitations / wrinkles

- **Adding a project requires re-running `kamaji up`.** Bind-mounts can't be
  hot-added to a running container, so after registering a new project the user
  re-runs `kamaji up` (idempotent: it re-derives mounts and recreates the
  container; volumes persist, no data loss). Auto-remount on project change is a
  future improvement.
- **First-run ordering.** On the very first `up` there are no registered
  projects, so the container starts with the board only. The user registers a
  project in the browser/TUI, then re-runs `kamaji up` to mount it. Documented in
  the install flow.
- **macOS.** Podman/Docker run inside a Linux VM there, so identical-path mounts
  and performance differ. v1 targets Linux hosts; macOS is a documented
  limitation, not a supported configuration.
- **Remote browsing.** Because `derive_proxy_addr()` pins the iframe's public
  host to `127.0.0.1`, the board is correct only when browsed from the container
  host. Remote/multi-user access is explicitly out of scope (see below).

## Error handling

- Runtime missing → actionable install hint (podman or docker).
- Published port already in use → detected before run, named clearly.
- Image pull failure → message plus retry guidance; existing local image is
  reused if present.
- Missing credentials → the agent fails at runtime inside the session; surfaced
  via `kamaji logs`.
- Worktree-path break → cannot occur via the launcher (it always derives
  identical mount paths); documented for users of the raw Quadlet/Compose path.

## Distribution & install UX

- `curl install.sh | sh` installs the host `kamaji` binary, unchanged.
- `kamaji up` pulls the image and starts the container; open `localhost:8755`.
- `kamaji down` stops it; `kamaji logs` tails logs.
- Ship a **Podman Quadlet unit** (`kamaji.container`, for
  `systemctl --user start kamaji`) and a **Compose file** for users who prefer to
  manage the container directly. The launcher is sugar over the same conventions.

## Testing strategy

- **Existing tests unchanged.** `kamaji-core` and `kamajid` suites continue to
  pass; this change adds a transport/packaging layer, not domain logic.
- **Launcher pure logic, unit-tested via argv/string assertions** — the same
  pattern already used for the git and zellij command builders (`git.rs`,
  `zellij.rs`). Cover: runtime detection, deriving `-v` mounts from a project
  list, generating the container config (bind + worktree_base), and assembling
  the full run command. No container required in CI.
- **Image build in CI** — build the image and smoke-test that `kamajid
  --version`, `zellij --version`, and each agent CLI are present and runnable
  inside it.
- **`#[ignore]`d end-to-end smoke** that actually boots the image and probes
  `/healthz` on the published port, mirroring the existing ignored "real zellij"
  tests that run only with `--ignored` on a machine that has the runtime.

## Out of scope (YAGNI)

Remote/multi-user access, TLS, an auth layer, per-agent/per-ticket containers
(the daemon spawning sibling containers instead of zellij sessions — a
re-architecture that also forfeits zellij web's browser terminal), Windows
containers, and hot mount-add without a container restart.
