# confy

a config manager for linux/unix based systems including macos (unix) and windows.

simple tui for keeping track of all your config files in one place. no more hunting through ~/.config.



## features

* **organize with groups** - create folders to organize your configs (hyprland/, nvim/, etc)
* **collapsible groups** - expand/collapse groups to keep your view clean
* **search** - real-time fuzzy search through all your configs
* **multiple sort modes** - sort by name, date modified, or file size
* **open in $EDITOR** - edit files with one keypress
* **remembers last file** - quick access to recently edited configs
* **built-in file picker** - no external dependencies, navigate with vim keys
* **rollback** - automatic compressed backups before every edit, restore with `:rb`
* **custom colors** - set colors via config.json, supports hex and named colors
* **vim-style keybinds** - j/k navigation, command mode
* **lightweight and fast** - pure python with curses, zero dependencies
* **cross-platform** - works on linux, macos, bsd, windows

## installation

### from AUR (arch linux)

```bash
yay -S confy-tui
```

### manual install

```bash
git clone https://gitlab.com/phluxjr/confy.git
cd confy
chmod +x main.py
sudo ln -s $(pwd)/main.py /usr/local/bin/confy
# optionally install the man page
sudo install -Dm644 confy.1 /usr/share/man/man1/confy.1
```

## dependencies

* python3
* curses (included with python)

that's it. no ranger, no external tools.

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
* `:rm` - remove selected file from tracking (does not delete the file)
* `:l` - open last edited file
* `:rb` - rollback selected file to last backup

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
* `:cd` - change config directory (opens built-in file picker)
* `:cd reset` - reset to ~/.config
* `:q` - quit

### rollback

confy automatically saves a compressed backup of any file to `/tmp/<filename>.confbak` before you open it for editing. if you make a mess of your config, select the file and run `:rb` to restore it.

rollback can be disabled in config.json:
```json
"settings": {
  "rollback": false
}
```

### colors

customize colors in `~/.config/confy/config.json` under `settings.colors`. values can be named colors or hex codes:

```json
"settings": {
  "colors": {
    "bg":        "default",
    "fg":        "default",
    "highlight": "#cba6f7",
    "group":     "#89b4fa"
  }
}
```

named colors: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `default`, `lavender`, `pink`, `purple`, `orange`

hex colors require a terminal that supports 256 colors (most do).

### search mode

press `/` to enter search mode, then start typing:
- filters both files and groups in real-time
- case-insensitive matching
- `enter` to accept and keep filtering
- `esc` to clear search and show all files

### groups

groups are purely organizational - your actual config files stay in their original locations. groups help you organize your tracked configs into logical categories like "hyprland", "nvim", "shell", etc.

groups are collapsible - press `space` or `enter` on a group header to toggle.

## configuration file

confy stores everything in `~/.config/confy/config.json`. if you're upgrading from an older version with `tracked.json`, confy will automatically migrate it on first run.

full example config.json:
```json
{
  "groups": {
    "ungrouped": [],
    "hyprland": ["/home/user/.config/hypr/hyprland.conf"],
    "nvim": ["/home/user/.config/nvim/init.lua"]
  },
  "settings": {
    "rollback": true,
    "colors": {
      "bg": "default",
      "fg": "default",
      "highlight": "#cba6f7",
      "group": "#89b4fa"
    }
  }
}
```

## why confy?

tired of doing `cd ~/.config/whatever` a million times a day? same. confy keeps all your important configs in one list so you can jump to them instantly.

organize related configs into groups, search through everything, sort however you want, and open files in your editor with a single keypress. if you break something, roll it back.

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
:ac hyprland  # opens file picker, navigate to hyprland.conf
:ac nvim      # opens file picker, navigate to init.lua

# move existing files between groups
# (select file first, then)
:mg shell

# search for configs
/hypr         # shows only hyprland-related files

# sort by recently modified
:sort date
:reverse      # newest first

# oops, broke your config
:rb           # rollback to last backup
```

## tips

* set `export EDITOR=nvim` in your shell rc for your preferred editor
* use groups to organize by application (hyprland/, nvim/, kitty/)
* use `:sort date` to quickly find recently edited configs
* search with `/` to quickly jump to specific configs
* collapse groups you don't use often to keep view clean
* missing files show up in red so you know when a config has moved

## windows support

on windows, change the config directory to where you keep your configs:
```
:cd
# navigate to C:\Users\YourName\AppData\Local or wherever
```

## license

GPL-3.0-or-later

## contributing

prs welcome! this is a simple tool but if you have ideas for improvements, open an issue or submit a pr.

## man page

a man page is included. after installing via AUR it's available automatically:
```bash
man confy
```
