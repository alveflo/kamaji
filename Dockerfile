# syntax=docker/dockerfile:1

# ---- builder: compile kamajid (release) ----
FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p kamajid

# ---- runtime: node base (gives node+npm on debian-slim) ----
FROM node:22-bookworm-slim AS runtime

ARG ZELLIJ_VERSION=v0.43.1

# git for worktrees; curl/ca-certificates to fetch zellij; tini for PID 1 reaping.
# curl is purged after fetching zellij; ca-certificates + git + tini remain for runtime.
RUN apt-get update \
 && apt-get install -y --no-install-recommends git curl ca-certificates tini \
 && curl -fsSL "https://github.com/zellij-org/zellij/releases/download/${ZELLIJ_VERSION}/zellij-x86_64-unknown-linux-musl.tar.gz" \
      | tar -xz -C /usr/local/bin \
 && zellij --version \
 && apt-get purge -y curl \
 && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/*

# Agent CLIs. Package names are verified by the --version smoke below; if any
# changes upstream, the build fails loudly here rather than at runtime.
RUN npm install -g @anthropic-ai/claude-code @openai/codex @github/copilot \
 && npm cache clean --force \
 && claude --version && codex --version && copilot --version

COPY --from=builder /src/target/release/kamajid /usr/local/bin/kamajid

WORKDIR /root

EXPOSE 8755 8756
# tini reaps zombie agent/zellij children. --bind 0.0.0.0 so the board is
# reachable from the host; the proxy auto-derives 0.0.0.0:8756.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["kamajid", "serve", "--bind", "0.0.0.0:8755"]
