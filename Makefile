# Alias-only convenience wrapper. Bare-metal launch logic lives in
# ./leti-ai; container logic lives in docker-compose.yml. These targets
# are one-line delegations so a reader debugging launch reads exactly one
# file. Do not add orchestration here.
.PHONY: run run-mock clean help compose-up compose-down compose-build compose-logs

run: ; ./leti-ai
run-mock: ; ./leti-ai --mock
clean: ; ./leti-ai --clean
help: ; ./leti-ai --help

# Container workflow. `compose-up` needs a local override for host ports
# (cp docker-compose.override.yml.example docker-compose.override.yml) and
# an env file (--env-file .env.local). Pass EXTRA for ad-hoc flags.
compose-build: ; docker compose build
compose-up: ; docker compose up -d $(EXTRA)
compose-down: ; docker compose down
compose-logs: ; docker compose logs -f server
