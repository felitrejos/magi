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
	//go:embed scripts/started.sh
	startedScript string
	//go:embed scripts/session.sh
	sessionBody string
	//go:embed scripts/memory.sh
	memoryScript string
	//go:embed scripts/retire.sh
	retireScript string
	//go:embed scripts/project.sh
	projectScript string
	//go:embed scripts/task.sh
	taskBody string
	//go:embed scripts/load.sh
	loadScript string
	//go:embed scripts/watch.sh
	watchBody string
	//go:embed scripts/platform.sh
	platformBody string
	//go:embed scripts/trust.sh
	trustScript string
	//go:embed scripts/forwards.sh
	closeForwardsScript string
	//go:embed scripts/try.sh
	tryScript string

	sessionScript  = startedScript + sessionBody
	taskScript     = startedScript + loadScript + taskBody
	watchScript    = loadScript + watchBody
	platformScript = loadScript + platformBody
)
