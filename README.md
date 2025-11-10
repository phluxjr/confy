# confy

a config manager for linux/unix based systems including macos (unix).

simple tui for keeping track of all your config files in one place. no more hunting through ~/.config.

<img width="1918" height="1081" alt="image" src="https://github.com/user-attachments/assets/a6736759-d430-433f-b93a-cd319dc61277" />


## features

- track config files from anywhere
- open in $EDITOR with one keypress
- remembers last edited file
- vim-style keybinds
- lightweight and fast
- works on linux, macos, bsd, whatever

## installation

### from AUR (arch linux)
```bash
yay -S confy-tui
```

### manual install
```bash
git clone https://github.com/Phluxjr23/confy.git
...

## dependencies

- python3
- ranger (for file picker)
- curses (usually included with python)

## usage

just run `confy` in your terminal

### keybinds

- `j/k` or `arrow keys` - navigate
- `enter` - open selected file in $EDITOR
- `:` - enter command mode
- `q` - quit

### commands

- `:ac` - add config (opens ranger file picker)
- `:rm` - remove selected file
- `:l` - open last edited file
- `:q` - quit

## why confy?

tired of doing `cd ~/.config/whatever` a million times a day? same. confy keeps all your important configs in one list so you can jump to them instantly.

simple, fast, does one thing well.

## license

mit

## btw

i use arch btw
