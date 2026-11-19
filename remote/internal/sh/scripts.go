package sh

import _ "embed"

var (
	//go:embed scripts/transfer.sh
	transferScript string
	//go:embed scripts/tmux.sh
	tmuxScript string
)
