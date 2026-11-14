package main

import _ "embed"

var (
	//go:embed scripts/inspect.sh
	inspectScript string
	//go:embed scripts/secrets.sh
	secretsScript string
	//go:embed scripts/checkout.sh
	checkoutScript string
	//go:embed scripts/gitlinks.sh
	gitlinksScript string
	//go:embed scripts/submodule.sh
	submoduleScript string
	//go:embed scripts/lfs.sh
	lfsScript string
	//go:embed scripts/folder.sh
	folderScript string
	//go:embed scripts/clones.sh
	clonesScript string
	//go:embed scripts/hosts.sh
	hostsScript string
	//go:embed scripts/transfer.sh
	transferScript string
	//go:embed scripts/session.sh
	sessionScript string
	//go:embed scripts/memory.sh
	memoryScript string
	//go:embed scripts/tmux.sh
	tmuxScript string
	//go:embed scripts/retire.sh
	retireScript string
	//go:embed scripts/project.sh
	projectScript string
)
