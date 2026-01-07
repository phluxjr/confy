# confy

a config manager for linux/unix based systems including macos (unix) and windows.

simple tui for keeping track of all your config files in one place. no more hunting through ~/.config.

![confy-demo](https://github.com/user-attachments/assets/830e306f-9450-4ba0-8b44-06434cdd78f3)

## features

* **organize with groups** - create folders to organize your configs (hyprland/, nvim/, etc)
* **collapsible groups** - expand/collapse groups to keep your view clean
* **search** - real-time fuzzy search through all your configs
* **multiple sort modes** - sort by name, date modified, or file size
* **open in $EDITOR** - edit files with one keypress
* **remembers last file** - quick access to recently edited configs
* **customizable config dir** - change base directory for file picker
* **vim-style keybinds** - j/k navigation, command mode
* **lightweight and fast** - pure python with curses
* **cross-platform** - works on linux, macos, bsd, windows

## installation

### from AUR (arch linux)

```bash
yay -S confy-tui
```

### manual install

```bash
git clone https://github.com/Phluxjr23/confy.git
cd confy
chmod +x main.py
# optionally symlink to PATH
sudo ln -s $(pwd)/main.py /usr/local/bin/confy
```

## dependencies

* python3
* ranger (for file picker)
* curses (usually included with python)

## usage

just run `confy` in your terminal

### navigation

* `j/k` or `arrow keys` - move up/down
* `enter` - open file in $EDITOR (or toggle group)
* `space` - toggle group expand/collapse
* `/` - search mode
* `:` - command mode
* `q` - quit

### commands

#### file management
* `:ac` - add config to ungrouped
* `:ac <group>` - add config to specific group
* `:rm` - remove selected file
* `:l` - open last edited file

#### group management
* `:ag <group>` - add new group
* `:mg <group>` - move selected file to group
* `:rg <group>` - remove group (moves files to ungrouped)

#### sorting & filtering
* `:sort name` - sort alphabetically
* `:sort date` - sort by last modified
* `:sort size` - sort by file size
* `:reverse` - toggle ascending/descending order
* `/` then type - search files and groups in real-time

#### configuration
* `:cd` - change config directory (opens ranger)
* `:cd reset` - reset to ~/.config (or default)
* `:q` - quit

### search mode

press `/` to enter search mode, then start typing:
- filters both files and groups in real-time
- case-insensitive fuzzy matching
- `enter` to accept and keep filtering
- `esc` to clear search and show all files

### groups

groups are purely organizational - your actual config files stay in their original locations. groups help you organize your list of tracked configs into logical categories like "hyprland", "nvim", "shell", etc.

groups are collapsible - press `space` or `enter` on a group header to toggle.

## why confy?

tired of doing `cd ~/.config/whatever` a million times a day? same. confy keeps all your important configs in one list so you can jump to them instantly.

organize related configs into groups, search through everything, sort however you want, and open files in your editor with a single keypress.

simple, fast, does one thing well.

## examples

```bash
# start confy
confy

# create some groups
:ag hyprland
:ag nvim
:ag shell

# add configs to groups
:ac hyprland  # opens ranger, pick hyprland.conf
:ac nvim      # opens ranger, pick init.lua

# move existing files between groups
# (select file first, then)
:mg shell

# search for configs
/hypr         # shows only hyprland-related files

# sort by recently modified
:sort date
:reverse      # newest first

# change where ranger starts
:cd           # pick new directory
:cd reset     # back to default
```

## tips

* set `export EDITOR=nvim` in your shell rc for your preferred editor
* use groups to organize by application (hyprland/, nvim/, kitty/)
* use `:sort date` to quickly find recently edited configs
* search with `/` to quickly jump to specific configs
* collapse groups you don't use often to keep view clean

## windows support

on windows, change the config directory to where you keep your configs:
```
:cd
# navigate to C:\Users\YourName\AppData\Local or wherever
```

ranger should work on windows via WSL or you can modify the code to use a different file picker.

## license

mit

## contributing

prs welcome! this is a simple tool but if you have ideas for improvements, open an issue or submit a pr.

## btw

i use arch btw
