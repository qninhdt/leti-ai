# Alias-only convenience wrapper. ALL launch logic lives in ./openlet-ai —
# these targets are one-line delegations so a reader debugging launch reads
# exactly one file. Do not add orchestration here.
.PHONY: run run-mock clean help

run: ; ./openlet-ai
run-mock: ; ./openlet-ai --mock
clean: ; ./openlet-ai --clean
help: ; ./openlet-ai --help
