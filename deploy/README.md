# Running kamaji in a container by hand

`kamaji up` does all of this for you; these files are the raw equivalents.

**Required, or the board won't work:**
- The daemon must bind `0.0.0.0` (the image's CMD already does).
- Publish ports **8755** and **8756**; 8082 stays internal.
- Set `HOME` to your host home and mount everything at **identical paths**, so
  git worktrees and agent credentials resolve the same inside and out.
- For every registered project, mount **both** its root **and** its worktree
  base dir (default `<root>/../kamaji-worktrees`).
- Mount agent credentials (`~/.claude`, etc.) or pass API keys via env.

Podman (rootless) is recommended; under it, container-root maps to your
unprivileged user. Plain Docker maps container-root to host-root.
