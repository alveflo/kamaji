# kamajid daemon control.
#
# The daemon (kamajid) serves the browser board on http://127.0.0.1:8755 and the
# zellij terminal proxy on :8756. `make restart` is the one you usually want
# after pulling new code — it rebuilds and relaunches so the running process
# picks up the latest changes.
#
#   make start | stop | restart | status | logs

KAMAJID     := target/debug/kamajid
LOG         := /tmp/kamajid.log
BOARD_URL   := http://127.0.0.1:8755
# Where the daemon writes its pidfile (mirrors kamaji-core::paths::runtime_dir:
# $XDG_RUNTIME_DIR, else $XDG_CACHE_HOME, else ~/.cache — each + /kamaji).
RUNTIME_DIR := $(or $(XDG_RUNTIME_DIR),$(XDG_CACHE_HOME),$(HOME)/.cache)
PID_FILE    := $(RUNTIME_DIR)/kamaji/kamajid.pid

# Autostart (systemd user service): install a release kamajid to ~/.local/bin and
# a user unit so the board comes back automatically after login / reboot.
RELEASE_BIN := target/release/kamajid
BINDIR      := $(HOME)/.local/bin
UNIT_SRC    := packaging/systemd/kamajid.service
UNIT_DIR    := $(or $(XDG_CONFIG_HOME),$(HOME)/.config)/systemd/user
UNIT_DEST   := $(UNIT_DIR)/kamajid.service

.DEFAULT_GOAL := help

.PHONY: help start stop restart status logs install-service uninstall-service

help: ## List the daemon control targets
	@grep -E '^[a-z-]+:.*?## ' $(MAKEFILE_LIST) \
	  | awk 'BEGIN{FS=":.*?## "}{printf "  \033[1m%-8s\033[0m %s\n", $$1, $$2}'

start: ## Build, then start the daemon in the background (board :8755, proxy :8756)
	cargo build -p kamajid
	@nohup $(KAMAJID) serve >$(LOG) 2>&1 & \
	  echo "kamajid started (pid $$!) — board $(BOARD_URL), logs: $(LOG)"

stop: ## Stop the running daemon (pidfile, falling back to process name)
	@if [ -f "$(PID_FILE)" ] && kill "$$(cat $(PID_FILE))" 2>/dev/null; then \
	  echo "kamajid stopped (pid $$(cat $(PID_FILE)))"; \
	elif pkill -x kamajid 2>/dev/null; then \
	  echo "kamajid stopped"; \
	else \
	  echo "kamajid not running"; \
	fi

restart: ## Rebuild and relaunch the daemon (use after pulling new code)
	@$(MAKE) --no-print-directory stop
	@sleep 1
	@$(MAKE) --no-print-directory start

status: ## Report whether the daemon is responding
	@curl -fsS $(BOARD_URL)/healthz >/dev/null 2>&1 \
	  && echo "kamajid is up — $(BOARD_URL)" \
	  || echo "kamajid is not responding"

logs: ## Follow the daemon log
	@tail -f $(LOG)

install-service: ## Install kamajid as a systemd user service (auto-starts on login/boot)
	cargo build --release -p kamajid
	@install -Dm755 $(RELEASE_BIN) $(BINDIR)/kamajid
	@install -Dm644 $(UNIT_SRC) $(UNIT_DEST)
	@systemctl --user daemon-reload
	@systemctl --user enable --now kamajid.service
	@loginctl enable-linger 2>/dev/null \
	  && echo "lingering enabled — kamajid will also start at boot, before login" \
	  || echo "note: run 'sudo loginctl enable-linger $$(id -un)' to start kamajid at boot (before login)"
	@echo "kamajid installed to $(BINDIR)/kamajid and running — board $(BOARD_URL)"

uninstall-service: ## Stop and remove the kamajid systemd user service
	-@systemctl --user disable --now kamajid.service 2>/dev/null
	-@rm -f $(UNIT_DEST)
	@systemctl --user daemon-reload
	@echo "kamajid service removed ($(BINDIR)/kamajid left in place)"
