Output "assets/handoff.gif"
Require claude
Set Shell "zsh"
Set FontSize 15
Set Width 1100
Set Height 620
Set Padding 18
Set Theme "Catppuccin Mocha"
Set TypingSpeed 35ms
Set WindowBar Colorful
Hide
Type "for v in ${(k)parameters[(I)CLAUDE*]}; do unset $v; done; export XDG_CONFIG_HOME=$PWD/assets/demo-config PROMPT='%F{yellow}macbook%f %F{blue}~/demo-app%f $ ' && cd ~/demo-app && clear"
Enter
Sleep 1s
Show
Type "claude"
Enter
Wait+Screen@30s /for shortcuts|Try "/
Sleep 1s
Type "What does this project do? One sentence."
Enter
Sleep 20s
Type "Hand this off to my homelab."
Enter
Wait@120s /macbook/
Sleep 2s
Type "ssh -t homelab tmux attach -t demo:demo-app"
Enter
Wait+Screen@30s /homelab/
Sleep 3s
Type "What did you tell me this project does?"
Enter
Sleep 22s
Hide
Ctrl+B
Type "d"
