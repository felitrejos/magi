.PHONY: dev

dev:
	mkdir -p "$$HOME/.magi/bin"
	cd remote && go build -o "$$HOME/.magi/bin/magi-dev" .
	ln -sfn magi-dev "$$HOME/.magi/bin/magi"
	@echo "Set MAGI_VERSION=dev where Claude Code starts so the plugin runs this build."
