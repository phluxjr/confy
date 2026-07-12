#!/usr/bin/env python3
# confy - a config manager for linux/unix systems
# Copyright (C) 2025-2026 phluxjr
# Licensed under GPL-3.0-or-later
import curses
import json
import os
import gzip
import re
import shutil
import subprocess
from pathlib import Path
from datetime import datetime

CONFIG_DIR = Path.home() / ".config" / "confy"
TRACKED_FILE = CONFIG_DIR / "tracked.json"  # legacy
CONFIG_FILE  = CONFIG_DIR / "config.json"
MOUNT_ROOT   = Path.home() / ".cache" / "confy" / "mounts"  # sshfs mountpoints live here
SSHFS_URL    = "https://github.com/libfuse/sshfs"

# ── default app config (user can override in config.json under "settings") ────

DEFAULT_SETTINGS = {
    "rollback": True,
    "first_startup": True,
    "theme": "catppuccin",
    "colors": {
        "bg":        "default",   # terminal default or hex like "#1e1e2e"
        "fg":        "default",
        "highlight": "#cba6f7",   # catppuccin mauve as default lol
        "group":     "#89b4fa",   # catppuccin blue
        "border":    "default",
    }
}

# ── built-in themes (:theme <name>) ─────────────────────────────────────────
# each theme is a full colors dict, same shape as DEFAULT_SETTINGS["colors"]
THEMES = {
    "catppuccin": {
        "bg": "default", "fg": "default",
        "highlight": "#cba6f7", "group": "#89b4fa", "border": "default",
    },
    "dracula": {
        "bg": "default", "fg": "default",
        "highlight": "#bd93f9", "group": "#8be9fd", "border": "default",
    },
    "gruvbox": {
        "bg": "default", "fg": "default",
        "highlight": "#fabd2f", "group": "#83a598", "border": "default",
    },
    "nord": {
        "bg": "default", "fg": "default",
        "highlight": "#88c0d0", "group": "#81a1c1", "border": "default",
    },
    "tokyo-night": {
        "bg": "default", "fg": "default",
        "highlight": "#bb9af7", "group": "#7aa2f7", "border": "default",
    },
    "one-dark": {
        "bg": "default", "fg": "default",
        "highlight": "#c678dd", "group": "#61afef", "border": "default",
    },
}

# ── color helpers ─────────────────────────────────────────────────────────────

# named terminal colors → curses color number
NAMED_COLORS = {
    "black":   curses.COLOR_BLACK,
    "red":     curses.COLOR_RED,
    "green":   curses.COLOR_GREEN,
    "yellow":  curses.COLOR_YELLOW,
    "blue":    curses.COLOR_BLUE,
    "magenta": curses.COLOR_MAGENTA,
    "cyan":    curses.COLOR_CYAN,
    "white":   curses.COLOR_WHITE,
    "default": -1,
    # some fun extras
    "lavender": curses.COLOR_CYAN,   # close enough lol
    "pink":     curses.COLOR_MAGENTA,
    "orange":   curses.COLOR_YELLOW,
    "purple":   curses.COLOR_MAGENTA,
}

def hex_to_curses(hex_color):
    """convert #rrggbb to curses 0-1000 rgb values"""
    hex_color = hex_color.lstrip('#')
    if len(hex_color) == 6:
        r = int(hex_color[0:2], 16) * 1000 // 255
        g = int(hex_color[2:4], 16) * 1000 // 255
        b = int(hex_color[4:6], 16) * 1000 // 255
        return r, g, b
    return None

def init_colors(color_settings):
    """initialise curses color pairs from settings"""
    curses.start_color()
    curses.use_default_colors()

    pairs = {}

    def resolve(c):
        """returns a curses color number, initialising custom colors if needed"""
        if c is None or c == "default":
            return -1
        cl = c.lower()
        if cl in NAMED_COLORS:
            return NAMED_COLORS[cl]
        if cl.startswith('#') and curses.can_change_color():
            rgb = hex_to_curses(cl)
            if rgb:
                # use high color numbers to avoid clobbering defaults
                color_num = 16 + len(pairs)
                try:
                    curses.init_color(color_num, *rgb)
                    return color_num
                except:
                    pass
        return -1

    fg  = resolve(color_settings.get("fg", "default"))
    bg  = resolve(color_settings.get("bg", "default"))
    hi  = resolve(color_settings.get("highlight", "default"))
    grp = resolve(color_settings.get("group", "default"))

    curses.init_pair(1, fg,  bg)   # normal
    curses.init_pair(2, hi,  bg)   # highlighted/selected
    curses.init_pair(3, grp, bg)   # group headers
    curses.init_pair(4, curses.COLOR_RED, bg)  # errors/missing

    return {
        "normal":    curses.color_pair(1),
        "highlight": curses.color_pair(2) | curses.A_REVERSE,
        "group":     curses.color_pair(3) | curses.A_BOLD,
        "error":     curses.color_pair(4),
    }

# ── built-in file picker (replaces ranger) ────────────────────────────────────

class FilePicker:
    def __init__(self, start_dir, colors):
        self.cwd = Path(start_dir).expanduser()
        self.selected = 0
        self.colors = colors
        self.entries = []
        self.scroll = 0

    def load_entries(self):
        try:
            entries = sorted(self.cwd.iterdir(), key=lambda p: (not p.is_dir(), p.name.lower()))
            self.entries = [self.cwd.parent] + entries  # ".." at top
        except PermissionError:
            self.entries = [self.cwd.parent]

    def run(self, stdscr, pick_dir=False):
        """
        run the file picker.
        pick_dir=False → returns selected file path (original behaviour)
        pick_dir=True  → returns the cwd when the user quits (for :cd)
        """
        self.load_entries()
        curses.curs_set(0)

        while True:
            stdscr.clear()
            h, w = stdscr.getmaxyx()

            if pick_dir:
                stdscr.addstr(0, 1, f"set config dir: {self.cwd}", self.colors["group"])
                stdscr.addstr(1, 1, "enter=open dir  q/esc=select this dir  backspace=up", self.colors["normal"])
            else:
                stdscr.addstr(0, 1, f"pick a file: {self.cwd}", self.colors["group"])
                stdscr.addstr(1, 1, "enter=open/select  q=cancel  backspace=up", self.colors["normal"])
            stdscr.addstr(2, 0, "─" * w, self.colors["normal"])

            visible = h - 5
            if self.selected < self.scroll:
                self.scroll = self.selected
            elif self.selected >= self.scroll + visible:
                self.scroll = self.selected - visible + 1

            for i, entry in enumerate(self.entries[self.scroll:self.scroll + visible]):
                y = 3 + i
                idx = i + self.scroll
                is_dir = entry.is_dir() if entry != self.cwd.parent else True
                name = "../" if entry == self.cwd.parent else (entry.name + ("/" if is_dir else ""))
                attr = self.colors["highlight"] if idx == self.selected else (
                    self.colors["group"] if is_dir else self.colors["normal"]
                )
                stdscr.addstr(y, 2, name[:w-3], attr)

            stdscr.refresh()
            key = stdscr.getch()

            if key in (ord('q'), 27):
                if pick_dir:
                    return str(self.cwd)
                return None
            elif key in (ord('j'), curses.KEY_DOWN):
                if self.selected < len(self.entries) - 1:
                    self.selected += 1
            elif key in (ord('k'), curses.KEY_UP):
                if self.selected > 0:
                    self.selected -= 1
            elif key in (ord('\n'), curses.KEY_ENTER):
                entry = self.entries[self.selected]
                if entry == self.cwd.parent or entry.is_dir():
                    self.cwd = entry.resolve()
                    self.selected = 0
                    self.scroll = 0
                    self.load_entries()
                else:
                    if pick_dir:
                        # selected a file in dir mode — just use the containing dir
                        return str(self.cwd)
                    return str(entry)
            elif key in (curses.KEY_BACKSPACE, 127, 8):
                self.cwd = self.cwd.parent
                self.selected = 0
                self.scroll = 0
                self.load_entries()

# ── device mode (remote profiles over sshfs) ───────────────────────────────

def parse_ssh_config():
    """parse ~/.ssh/config for Host aliases -> resolved user@hostname.
    returns a dict like {"phluxjr": "will@phluxjr.net"}. best-effort, never raises."""
    ssh_config_path = Path.home() / ".ssh" / "config"
    aliases = {}
    if not ssh_config_path.exists():
        return aliases
    try:
        current_hosts = []
        current_hostname = None
        current_user = None

        def flush():
            if not current_hosts:
                return
            hostname = current_hostname
            user = current_user
            for h in current_hosts:
                if h == "*":
                    continue
                resolved_host = hostname or h
                if user:
                    aliases[h] = f"{user}@{resolved_host}"
                else:
                    aliases[h] = resolved_host

        with open(ssh_config_path, 'r') as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith('#'):
                    continue
                parts = line.split(None, 1)
                if len(parts) != 2:
                    continue
                key, val = parts[0].lower(), parts[1].strip()
                if key == "host":
                    flush()
                    current_hosts = val.split()
                    current_hostname = None
                    current_user = None
                elif key == "hostname":
                    current_hostname = val
                elif key == "user":
                    current_user = val
            flush()
    except Exception:
        pass
    return aliases

def resolve_device_target(arg):
    """resolve a :device argument to a user@host string.
    checks ssh config aliases first, falls back to treating arg as literal."""
    aliases = parse_ssh_config()
    if arg in aliases:
        return aliases[arg]
    return arg

def sshfs_available():
    return shutil.which("sshfs") is not None

class DeviceManager:
    """handles mounting/unmounting a remote host's confy config dir via sshfs"""

    @staticmethod
    def _is_mounted(mountpoint):
        """check if something is already mounted at this path (via /proc/mounts, linux-only
        but that's the realistic target here; falls back to False on other platforms)"""
        try:
            with open('/proc/mounts', 'r') as f:
                return any(str(mountpoint) in line for line in f)
        except Exception:
            return False

    @staticmethod
    def _clear_stale_mount(mountpoint):
        """best-effort cleanup of a leftover mount/broken mountpoint from a previous
        session (crash, kill -9, unclean shutdown, etc). returns (ok, message)."""
        if not mountpoint.exists():
            return True, None

        if DeviceManager._is_mounted(mountpoint):
            # something's still attached here, try to unmount it first
            if not DeviceManager.unmount(mountpoint):
                return False, (
                    f"a stale mount is stuck at {mountpoint} and couldn't be freed. "
                    f"try manually: fusermount -u {mountpoint}"
                )

        # after unmounting (or if it was never mounted), confirm we can actually touch it.
        # a mountpoint left behind by a crashed sshfs process can end up owned/locked in a
        # way that makes even the owning user unable to write to it.
        try:
            test_file = mountpoint / ".confy_write_test"
            test_file.touch()
            test_file.unlink()
        except PermissionError:
            return False, (
                f"can't access {mountpoint} (leftover from a previous mount?). "
                f"try manually: fusermount -u {mountpoint} && rm -rf {mountpoint}"
            )
        except Exception as e:
            return False, f"couldn't verify mountpoint {mountpoint}: {e}"

        return True, None

    @staticmethod
    def resolve_remote_home(target):
        """ssh in briefly to ask the remote shell for $HOME. returns (ok, home_path_or_error)"""
        try:
            result = subprocess.run(
                ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", target, "echo $HOME"],
                capture_output=True, text=True, timeout=15
            )
            if result.returncode != 0:
                err = result.stderr.strip() or "ssh connection failed"
                return False, err
            home = result.stdout.strip()
            if not home:
                return False, "remote returned an empty $HOME"
            return True, home
        except subprocess.TimeoutExpired:
            return False, "ssh timed out resolving remote home dir"
        except FileNotFoundError:
            return False, "ssh not found on this system"
        except Exception as e:
            return False, f"ssh error: {e}"

    @staticmethod
    def mount(target, mount_name):
        """mount the remote host's entire root filesystem (/) to a local mountpoint,
        so absolute tracked paths (like /etc/jail.conf) resolve correctly through the
        mount instead of colliding with local paths of the same name.
        returns (success, message, mountpoint_path_or_None, remote_home_or_None)"""
        if not sshfs_available():
            return False, f"sshfs not found. install it: {SSHFS_URL}", None, None

        ok, home_or_err = DeviceManager.resolve_remote_home(target)
        if not ok:
            return False, f"couldn't reach {target}: {home_or_err}", None, None
        remote_home = home_or_err

        mountpoint = MOUNT_ROOT / mount_name

        try:
            mountpoint.mkdir(parents=True, exist_ok=True)
        except PermissionError:
            return False, f"can't create mountpoint {mountpoint} (permission denied)", None, None

        ok, err = DeviceManager._clear_stale_mount(mountpoint)
        if not ok:
            return False, err, None, None

        # mount the remote's root filesystem, not just the confy dir, so absolute paths
        # like /etc/jail.conf resolve to <mountpoint>/etc/jail.conf and both stat and
        # $EDITOR see the real remote file instead of a same-named local one
        remote_path = f"{target}:/"

        # allow_root is required so that a root process (e.g. via pkexec for :su)
        # can actually see/write through this mount. without it, FUSE only exposes
        # the mount to the mounting user, and root gets an empty/inaccessible view,
        # which is exactly the "empty read-only buffer" failure mode :su hit.
        base_opts = "reconnect,ServerAliveInterval=15,ServerAliveCountMax=3"
        su_will_work = True

        try:
            result = subprocess.run(
                ["sshfs", remote_path, str(mountpoint), "-o", f"{base_opts},allow_root"],
                capture_output=True, text=True, timeout=20
            )
            if result.returncode != 0:
                err = result.stderr.strip() or "unknown sshfs error"
                if "allow_root" in err or "user_allow_other" in err:
                    # local system doesn't have user_allow_other in /etc/fuse.conf.
                    # fall back to a normal mount so :device still works, just without :su.
                    su_will_work = False
                    result = subprocess.run(
                        ["sshfs", remote_path, str(mountpoint), "-o", base_opts],
                        capture_output=True, text=True, timeout=20
                    )
                    if result.returncode != 0:
                        err2 = result.stderr.strip() or "unknown sshfs error"
                        return False, f"sshfs failed: {err2}", None, None
                else:
                    return False, f"sshfs failed: {err}", None, None
        except subprocess.TimeoutExpired:
            return False, "sshfs timed out, check host/network", None, None
        except FileNotFoundError:
            return False, f"sshfs not found. install it: {SSHFS_URL}", None, None
        except Exception as e:
            return False, f"sshfs error: {e}", None, None

        msg = "mounted" if su_will_work else (
            "mounted (:su won't work here, add 'user_allow_other' to /etc/fuse.conf to enable it)"
        )
        return True, msg, mountpoint, remote_home

    @staticmethod
    def to_local_path(mountpoint, remote_abs_path):
        """translate an absolute remote path (as stored in the remote's config.json,
        e.g. /etc/jail.conf) to its local location under the root-mounted sshfs tree."""
        # strip a leading slash so it joins cleanly under the mountpoint
        rel = str(remote_abs_path).lstrip('/')
        return mountpoint / rel

    @staticmethod
    def unmount(mountpoint):
        """unmount a previously mounted device. tries fusermount, falls back to umount."""
        try:
            result = subprocess.run(["fusermount", "-u", str(mountpoint)],
                                     capture_output=True, text=True, timeout=10)
            if result.returncode == 0:
                return True
        except Exception:
            pass
        try:
            result = subprocess.run(["umount", str(mountpoint)],
                                     capture_output=True, text=True, timeout=10)
            return result.returncode == 0
        except Exception:
            return False

    @staticmethod
    def check_remote_format(confy_dir):
        """inspect the remote's confy dir (already resolved through the mount) to sanity-check
        it before switching over. returns one of: 'ok' (config.json present),
        'legacy' (only tracked.json, old version), 'empty' (neither exists, fresh remote),
        'unreachable' (can't even list the dir)"""
        try:
            entries = os.listdir(confy_dir)
        except Exception:
            return "unreachable"
        if "config.json" in entries:
            return "ok"
        if "tracked.json" in entries:
            return "legacy"
        return "empty"

# ── tutorial popup sequence ───────────────────────────────────────────────────

TUTORIAL_STEPS = [
    (
        "welcome to confy!",
        [
            "confy tracks your config files in one place.",
            "use j/k (or arrow keys) to move up and down.",
            "press enter to open a file in your $EDITOR.",
            "press p to toggle a live preview pane.",
            "",
            "press any key for the next tip...",
        ]
    ),
    (
        "groups",
        [
            "files are organised into groups.",
            "press enter or space on a group to collapse/expand it.",
            "",
            ":ag <name>  →  add a group",
            ":rg <name>  →  remove a group (files go to ungrouped)",
            ":mg <name>  →  move selected file to a group",
            "",
            "press any key for the next tip...",
        ]
    ),
    (
        "commands  (press : to enter)",
        [
            ":ac          →  add a config file (opens file picker)",
            ":ac <group>  →  add directly to a group",
            ":rm          →  remove selected file from tracking",
            ":l           →  reopen last edited file",
            ":cd          →  change the config search directory",
            ":sort name|date|size  →  sort files",
            ":reverse     →  flip sort order",
            ":theme <name>  →  switch color theme",
            ":device <host>  →  browse a remote host's configs",
            "  (needs sshfs)",
            ":device local  →  back to local configs",
            ":su  →  edit selected file as root (needs polkit)",
            ":h           →  show this tutorial again",
            ":help        →  show this tutorial again",
            "",
            "press any key for the next tip...",
        ]
    ),
    (
        "search  (press / to enter)",
        [
            "type to filter files and groups live.",
            "press enter to confirm, esc to clear.",
            "",
            "rollback  (:rb)",
            "confy saves a backup to /tmp whenever you",
            "open a file for editing. :rb restores it.",
            "set  \"rollback\": false  in config to disable.",
            "",
            "press any key for the next tip...",
        ]
    ),
    (
        "that's it!",
        [
            "tip: set your $EDITOR env var to your preferred",
            "editor (vim, nvim, nano, micro, etc.).",
            "",
            "for full docs run:  man confy",
            "(or just poke around, there isn't much to break!)",
            "",
            "press any key to start...",
        ]
    ),
]

def show_tutorial(stdscr, colors, draw_bg):
    """show the first-startup tutorial popup sequence overlaid on the main ui"""
    curses.curs_set(0)
    n = colors.get("normal", 0)
    g = colors.get("group", 0)

    for step_num, (title, lines) in enumerate(TUTORIAL_STEPS):
        # repaint the app behind the popup so it looks like an overlay
        draw_bg()

        h, w = stdscr.getmaxyx()

        content_w = min(62, w - 4)
        content_h = len(lines) + 4
        py = max(0, h // 2 - content_h // 2)
        px = max(0, w // 2 - content_w // 2)

        try:
            win = curses.newwin(content_h, content_w, py, px)
            win.bkgd(' ', n)
            win.box()
            # title in the top border
            title_str = f" {title} "
            win.addstr(0, max(1, (content_w - len(title_str)) // 2), title_str, g)
            # step counter in bottom border
            counter = f" {step_num + 1}/{len(TUTORIAL_STEPS)} "
            win.addstr(content_h - 1, max(1, content_w - len(counter) - 1), counter, n)
            # content lines
            for i, line in enumerate(lines):
                win.addstr(2 + i, 2, line[:content_w - 4], n)
            win.refresh()
        except:
            pass

        stdscr.getch()

# ── main app ──────────────────────────────────────────────────────────────────

class Confy:
    def __init__(self):
        self.groups = {"ungrouped": []}
        self.selected = 0
        self.page = 0
        self.command_mode = False
        self.search_mode = False
        self.command_buffer = ""
        self.search_buffer = ""
        self.last_opened = None
        self.config_dir = str(Path.home() / ".config")
        self.collapsed_groups = set()
        self.flat_view = []
        self.sort_mode = "name"
        self.sort_order = "asc"
        self.settings = dict(DEFAULT_SETTINGS)
        self.popup_message = None
        self.colors = {}
        self.show_tutorial = False  # resolved after load_data
        self.preview_enabled = False  # toggled with 'p', persisted in settings
        # ── device mode (remote profiles over sshfs) ──
        self.active_config_dir = CONFIG_DIR    # repointed at a mount when a device is active
        self.active_config_file = CONFIG_FILE
        self.device_name = None                # None = local, else display name like "phluxjr"
        self.device_mountpoint = None           # Path to the root-mounted sshfs tree, if any
        self.device_remote_home = None          # remote $HOME, needed to locate its confy dir under the mount
        self.device_su_available = True         # whether :su works on the current mount (fuse.conf dependent)
        self._local_state = None                # stashed local state while a device is mounted
        self.migrate_if_needed()
        self.load_data()
        self.rebuild_flat_view()

    def items_per_page(self, stdscr):
        """compute how many items fit given current terminal height"""
        h, _ = stdscr.getmaxyx()
        # header: rows 0-2 (3 rows), footer: rows h-2, h-1 (2 rows), row 3 is spacer
        # usable rows start at 4, end at h-3 inclusive
        return max(1, h - 6)

    def migrate_if_needed(self):
        """migrate tracked.json -> config.json if needed, for the currently active config location"""
        self.active_config_dir.mkdir(parents=True, exist_ok=True)
        legacy = self.active_config_dir / "tracked.json"
        if not self.active_config_file.exists() and legacy.exists():
            shutil.copy(legacy, self.active_config_file)
            self.popup_message = "migrated tracked.json → config.json!"

    def _reset_to_defaults(self):
        """reset in-memory state to a blank slate. used whenever load_data decides
        there's nothing valid to load (missing/empty/corrupt config), so stale state
        from a previous session (e.g. local, before a :device switch) doesn't linger."""
        self.groups = {"ungrouped": []}
        self.last_opened = None
        self.collapsed_groups = set()
        self.sort_mode = "name"
        self.sort_order = "asc"
        self.settings = dict(DEFAULT_SETTINGS)
        self.preview_enabled = False
        # note: config_dir is intentionally NOT reset here, it's expected to already be
        # set to a sane default by the caller (local home, or remote home on a device)

    def load_data(self):
        if not self.active_config_file.exists():
            # genuine first run — no config file yet
            self._reset_to_defaults()
            self.show_tutorial = True
            return
        try:
            with open(self.active_config_file, 'r') as f:
                raw = f.read()
            if not raw.strip():
                # file exists but is empty (e.g. mid-write, or a fresh touch'd file)
                self.popup_message = f"{self.active_config_file.name} is empty, starting fresh"
                self._reset_to_defaults()
                self.show_tutorial = True
                return
            data = json.loads(raw)
        except json.JSONDecodeError as e:
            self.popup_message = f"couldn't parse {self.active_config_file.name}: {e}"
            self._reset_to_defaults()
            self.show_tutorial = True
            return
        except OSError as e:
            self.popup_message = f"couldn't read {self.active_config_file.name}: {e}"
            self._reset_to_defaults()
            self.show_tutorial = True
            return

        if 'files' in data:
            self.groups = {"ungrouped": data['files']}
        else:
            self.groups = data.get('groups', {"ungrouped": []})
        self.last_opened = data.get('last_opened')
        self.collapsed_groups = set(data.get('collapsed_groups', []))
        self.sort_mode = data.get('sort_mode', 'name')
        self.sort_order = data.get('sort_order', 'asc')
        self.config_dir = data.get('config_dir', self.config_dir or str(Path.home() / ".config"))
        # load user settings, merging with defaults
        user_settings = data.get('settings', {})
        self.settings.update(user_settings)
        if 'colors' in user_settings:
            self.settings['colors'] = {**DEFAULT_SETTINGS['colors'], **user_settings['colors']}
        # show tutorial only if explicitly flagged true in saved config
        self.show_tutorial = self.settings.get('first_startup', False)
        self.preview_enabled = data.get('preview_enabled', False)

    def save_data(self):
        self.active_config_dir.mkdir(parents=True, exist_ok=True)
        with open(self.active_config_file, 'w') as f:
            json.dump({
                'groups': self.groups,
                'last_opened': self.last_opened,
                'collapsed_groups': list(self.collapsed_groups),
                'sort_mode': self.sort_mode,
                'sort_order': self.sort_order,
                'config_dir': self.config_dir,
                'settings': self.settings,
                'preview_enabled': self.preview_enabled,
            }, f, indent=2)

    # ── themes ────────────────────────────────────────────────────────────────

    def set_theme(self, name):
        """apply a built-in theme by name, persist it, and re-init curses colors live"""
        name = name.strip().lower()
        if name not in THEMES:
            names = ", ".join(THEMES.keys())
            self.popup_message = f"unknown theme '{name}'. options: {names}"
            return
        self.settings['theme'] = name
        self.settings['colors'] = dict(THEMES[name])
        self.save_data()
        # re-init colors immediately so it applies without restarting
        self.colors = init_colors(self.settings['colors'])
        self.popup_message = f"theme → {name}"

    # ── device mode (remote profiles) ────────────────────────────────────────

    def _snapshot_state(self):
        """capture the in-memory state that's swapped when entering/leaving device mode"""
        return {
            'groups': self.groups,
            'last_opened': self.last_opened,
            'collapsed_groups': self.collapsed_groups,
            'sort_mode': self.sort_mode,
            'sort_order': self.sort_order,
            'config_dir': self.config_dir,
            'settings': self.settings,
            'preview_enabled': self.preview_enabled,
            'active_config_dir': self.active_config_dir,
            'active_config_file': self.active_config_file,
        }

    def _restore_state(self, snap):
        self.groups = snap['groups']
        self.last_opened = snap['last_opened']
        self.collapsed_groups = snap['collapsed_groups']
        self.sort_mode = snap['sort_mode']
        self.sort_order = snap['sort_order']
        self.config_dir = snap['config_dir']
        self.settings = snap['settings']
        self.preview_enabled = snap['preview_enabled']
        self.active_config_dir = snap['active_config_dir']
        self.active_config_file = snap['active_config_file']

    def resolve_local_path(self, tracked_path):
        """translate a tracked file's path into the path confy should actually stat/open.
        locally this is a no-op; on a mounted device, absolute remote paths get rewritten
        to live under the root-mounted sshfs tree (e.g. /etc/jail.conf ->
        <mountpoint>/etc/jail.conf) so stat/open see the real remote file."""
        if self.device_mountpoint is None:
            return tracked_path
        return str(DeviceManager.to_local_path(self.device_mountpoint, tracked_path))

    def resolve_tracked_path(self, local_path):
        """reverse of resolve_local_path: given a path the file picker returned (which,
        on a mounted device, lives under the sshfs mountpoint), convert it back to the
        original remote absolute path for storage in config.json. locally this is a no-op."""
        if self.device_mountpoint is None:
            return local_path
        try:
            rel = Path(local_path).relative_to(self.device_mountpoint)
            return "/" + str(rel)
        except ValueError:
            # picked path wasn't under the mount somehow, store as-is rather than crash
            return local_path

    def switch_device(self, arg, stdscr):
        """handle :device / :ssh <target-or-alias>. mounts the remote host's root filesystem
        over sshfs and switches the live view to its tracked configs. 'local' switches back."""
        arg = arg.strip()
        if not arg or arg.lower() == "local":
            self.switch_to_local()
            return

        if not sshfs_available():
            self.popup_message = f"sshfs not found. install it: {SSHFS_URL}"
            return

        target = resolve_device_target(arg)
        mount_name = re.sub(r'[^a-zA-Z0-9_.-]', '_', target)

        self.popup_message = f"connecting to {target}..."
        self.draw(stdscr)

        # if we're already on a device, unmount it cleanly first
        if self.device_mountpoint is not None:
            DeviceManager.unmount(self.device_mountpoint)

        ok, msg, mountpoint, remote_home = DeviceManager.mount(target, mount_name)
        if not ok:
            self.popup_message = msg
            return
        # mount() returns a descriptive success message when :su won't work on this
        # mount (fuse.conf doesn't allow_root locally); "mounted" means everything's fine
        device_su_available = (msg == "mounted")

        confy_dir = DeviceManager.to_local_path(mountpoint, f"{remote_home}/.config/confy")

        fmt = DeviceManager.check_remote_format(confy_dir)
        if fmt == "unreachable":
            DeviceManager.unmount(mountpoint)
            self.popup_message = f"couldn't read remote confy dir on {target}"
            return
        if fmt == "legacy":
            DeviceManager.unmount(mountpoint)
            self.popup_message = (
                f"{target} is running an older confy (tracked.json, no config.json). "
                f"update confy on that host first."
            )
            return
        # fmt is "ok" or "empty" -- both fine to proceed with

        # stash local state on first hop into device mode
        if self._local_state is None:
            self._local_state = self._snapshot_state()

        self.device_name = arg if arg in parse_ssh_config() else target
        self.device_mountpoint = mountpoint
        self.device_remote_home = remote_home
        self.device_su_available = device_su_available
        self.active_config_dir = confy_dir
        self.active_config_file = confy_dir / "config.json"
        # sane default in case the remote's config.json doesn't specify one (fresh remote)
        self.config_dir = f"{remote_home}/.config"

        # reset transient view state and load the remote data fresh
        self.selected = 0
        self.page = 0
        self.search_buffer = ""
        self.search_mode = False
        self.settings = dict(DEFAULT_SETTINGS)
        self.popup_message = None
        self.migrate_if_needed()
        self.load_data()
        self.rebuild_flat_view()

        if self.popup_message:
            # load_data hit a parse/read error, surface that instead of a fake success message
            self.popup_message = f"device → {self.device_name}, but {self.popup_message}"
        elif not device_su_available:
            self.popup_message = f"device → {self.device_name}. note: {msg}"
        else:
            note = "" if fmt == "ok" else " (empty, nothing tracked there yet)"
            self.popup_message = f"device → {self.device_name}{note}"

    def switch_to_local(self):
        """unmount the active device (if any) and restore local state"""
        if self.device_mountpoint is None:
            self.popup_message = "already local"
            return

        DeviceManager.unmount(self.device_mountpoint)
        self.device_mountpoint = None
        self.device_remote_home = None
        self.device_name = None
        self.device_su_available = True

        if self._local_state is not None:
            self._restore_state(self._local_state)
            self._local_state = None

        self.selected = 0
        self.page = 0
        self.rebuild_flat_view()
        self.popup_message = "device → local"

    # ── rollback ──────────────────────────────────────────────────────────────

    def save_backup(self, filepath):
        """save compressed backup of filepath to /tmp. filepath is the tracked
        (original/remote) path, translated internally to the local/mounted location."""
        if not self.settings.get('rollback', True):
            return
        local_path = self.resolve_local_path(filepath)
        try:
            bak_name = Path(filepath).name + ".confbak"
            bak_path = Path("/tmp") / bak_name
            with open(local_path, 'rb') as f_in:
                with gzip.open(bak_path, 'wb') as f_out:
                    shutil.copyfileobj(f_in, f_out)
        except Exception:
            pass

    def rollback(self, filepath):
        """restore backup for filepath, returns (success, message).
        filepath is the tracked (original/remote) path, translated internally."""
        local_path = self.resolve_local_path(filepath)
        bak_name = Path(filepath).name + ".confbak"
        bak_path = Path("/tmp") / bak_name
        if not bak_path.exists():
            return False, "no backup found in /tmp"
        try:
            with gzip.open(bak_path, 'rb') as f_in:
                with open(local_path, 'wb') as f_out:
                    shutil.copyfileobj(f_in, f_out)
            return True, f"rolled back {Path(filepath).name}!"
        except Exception as e:
            return False, f"rollback failed: {e}"

    def confirm_popup(self, stdscr, message):
        """show a confirmation popup, returns True if confirmed"""
        h, w = stdscr.getmaxyx()
        pw, ph = 50, 5
        py, px = h // 2 - ph // 2, w // 2 - pw // 2

        win = curses.newwin(ph, pw, py, px)
        win.box()
        win.addstr(1, 2, message[:pw-4], self.colors.get("normal", 0))
        win.addstr(3, 2, "y = confirm   n / esc = cancel", self.colors.get("group", 0))
        win.refresh()

        while True:
            key = stdscr.getch()
            if key == ord('y'):
                return True
            elif key in (ord('n'), 27, ord('q')):
                return False

    # ── sorting / view ────────────────────────────────────────────────────────

    def sort_files(self, files):
        if self.sort_mode == "name":
            sorted_files = sorted(files, key=lambda f: Path(f).name.lower())
        elif self.sort_mode == "date":
            sorted_files = sorted(files, key=lambda f: os.path.getmtime(self.resolve_local_path(f)) if os.path.exists(self.resolve_local_path(f)) else 0)
        elif self.sort_mode == "size":
            sorted_files = sorted(files, key=lambda f: os.path.getsize(self.resolve_local_path(f)) if os.path.exists(self.resolve_local_path(f)) else 0)
        else:
            sorted_files = files
        if self.sort_order == "desc":
            sorted_files = sorted_files[::-1]
        return sorted_files

    def rebuild_flat_view(self):
        self.flat_view = []
        if self.search_buffer:
            query = self.search_buffer.lower()
            for group_name in sorted(self.groups.keys()):
                matching_files = [f for f in self.groups[group_name]
                                  if query in Path(f).name.lower() or query in group_name.lower()]
                if matching_files or query in group_name.lower():
                    self.flat_view.append(('group', group_name))
                    if group_name not in self.collapsed_groups:
                        for filepath in self.sort_files(matching_files):
                            self.flat_view.append(('file', filepath, group_name))
        else:
            for group_name in sorted(self.groups.keys()):
                self.flat_view.append(('group', group_name))
                if group_name not in self.collapsed_groups:
                    for filepath in self.sort_files(self.groups[group_name]):
                        self.flat_view.append(('file', filepath, group_name))

    def get_file_info(self, filepath):
        """filepath is the tracked (original/remote) path; translated internally
        so this works transparently whether local or on a mounted device."""
        try:
            stat = os.stat(self.resolve_local_path(filepath))
            mtime = datetime.fromtimestamp(stat.st_mtime).strftime('%Y-%m-%d %H:%M')
            size = self.format_size(stat.st_size)
            return mtime, size
        except:
            return "unknown", "unknown"

    def format_size(self, size):
        for unit in ['B', 'KB', 'MB', 'GB']:
            if size < 1024:
                return f"{size:.1f}{unit}"
            size /= 1024
        return f"{size:.1f}TB"

    def get_preview_lines(self, filepath, max_lines, max_width):
        """read up to max_lines from filepath for the preview pane, truncating long lines.
        filepath is the tracked (original/remote) path; translated internally.
        returns a list of display strings, never raises."""
        local_path = self.resolve_local_path(filepath)
        try:
            if not os.path.exists(local_path):
                return ["(file not found)"]
            if os.path.isdir(local_path):
                return ["(directory, nothing to preview)"]
            if os.path.getsize(local_path) > 5 * 1024 * 1024:
                return ["(file too large to preview, 5MB+)"]
            lines = []
            with open(local_path, 'r', errors='replace') as f:
                for i, line in enumerate(f):
                    if i >= max_lines:
                        lines.append("...")
                        break
                    line = line.rstrip('\n')
                    if len(line) > max_width:
                        line = line[:max_width - 1] + "…"
                    lines.append(line)
            if not lines:
                return ["(empty file)"]
            return lines
        except UnicodeDecodeError:
            return ["(binary file, cannot preview)"]
        except PermissionError:
            return ["(permission denied)"]
        except Exception as e:
            return [f"(error reading file: {e})"]

    # ── file operations ───────────────────────────────────────────────────────

    def add_config(self, stdscr, group_name="ungrouped"):
        # when a device is mounted, browse the config_dir as it exists on the remote,
        # i.e. under the sshfs mountpoint, not confy's own local filesystem
        browse_root = self.resolve_local_path(self.config_dir) if self.device_mountpoint else self.config_dir
        picker = FilePicker(browse_root, self.colors)
        picked = picker.run(stdscr, pick_dir=False)
        if picked:
            filepath = self.resolve_tracked_path(picked)
            for grp_files in self.groups.values():
                if filepath in grp_files:
                    self.popup_message = "already tracked!"
                    return
            if group_name not in self.groups:
                self.groups[group_name] = []
            self.groups[group_name].append(filepath)
            self.save_data()
            self.rebuild_flat_view()

    def remove_config(self):
        if not self.flat_view or self.selected >= len(self.flat_view):
            return
        item = self.flat_view[self.selected]
        if item[0] == 'file':
            filepath = item[1]
            group_name = item[2]
            if filepath in self.groups[group_name]:
                self.groups[group_name].remove(filepath)
                self.save_data()
                self.rebuild_flat_view()
                if self.selected >= len(self.flat_view) and self.flat_view:
                    self.selected = len(self.flat_view) - 1

    def add_group(self, group_name):
        if group_name and group_name not in self.groups:
            self.groups[group_name] = []
            self.save_data()
            self.rebuild_flat_view()

    def remove_group(self, group_name):
        if group_name in self.groups and group_name != "ungrouped":
            self.groups["ungrouped"].extend(self.groups[group_name])
            del self.groups[group_name]
            self.save_data()
            self.rebuild_flat_view()

    def move_to_group(self, group_name):
        if not self.flat_view or self.selected >= len(self.flat_view):
            return
        item = self.flat_view[self.selected]
        if item[0] == 'file':
            filepath = item[1]
            old_group = item[2]
            if group_name not in self.groups:
                self.groups[group_name] = []
            if filepath in self.groups[old_group]:
                self.groups[old_group].remove(filepath)
            if filepath not in self.groups[group_name]:
                self.groups[group_name].append(filepath)
            self.save_data()
            self.rebuild_flat_view()

    def toggle_group(self):
        if not self.flat_view or self.selected >= len(self.flat_view):
            return
        item = self.flat_view[self.selected]
        if item[0] == 'group':
            group_name = item[1]
            if group_name in self.collapsed_groups:
                self.collapsed_groups.remove(group_name)
            else:
                self.collapsed_groups.add(group_name)
            self.save_data()
            self.rebuild_flat_view()

    def open_file(self, filepath):
        """filepath is the tracked (original/remote) path. the editor is launched on the
        translated local path so it edits the real file, whether local or mounted."""
        self.save_backup(filepath)
        local_path = self.resolve_local_path(filepath)
        editor = os.environ.get('EDITOR', 'nano')
        curses.endwin()
        try:
            subprocess.run([editor, local_path])
        except FileNotFoundError:
            input(f"error: editor '{editor}' not found. press enter...")
        except Exception as e:
            input(f"error opening file: {e}. press enter...")
        curses.doupdate()
        self.last_opened = filepath
        self.save_data()

    def open_file_elevated(self, filepath):
        """:su <selected file> — open with local root via pkexec. works on whatever
        path is currently in view, local or sshfs-mounted: pkexec only sees a plain
        local path either way, so this is 'local root, on the current path', nothing
        fancier. does not attempt to become a different user on a remote host."""
        if shutil.which("pkexec") is None:
            self.popup_message = "pkexec not found, can't elevate (needs polkit)"
            return

        if self.device_mountpoint is not None and not self.device_su_available:
            self.popup_message = (
                "can't :su through this mount, add 'user_allow_other' to "
                "/etc/fuse.conf and reconnect (:device local, then :device again)"
            )
            return

        self.save_backup(filepath)
        local_path = self.resolve_local_path(filepath)
        editor = os.environ.get('EDITOR', 'nano')
        editor_path = shutil.which(editor)
        if editor_path is None:
            self.popup_message = f"editor '{editor}' not found on PATH"
            return

        curses.endwin()
        try:
            result = subprocess.run(["pkexec", editor_path, local_path])
            if result.returncode != 0:
                input(f"pkexec exited with code {result.returncode} (auth cancelled/failed?). press enter...")
        except FileNotFoundError:
            input("error: pkexec not found. press enter...")
        except Exception as e:
            input(f"error opening file with pkexec: {e}. press enter...")
        curses.doupdate()
        self.last_opened = filepath
        self.save_data()

    def change_config_dir(self, stdscr, direct_path=None):
        """change the config search directory used by the file picker"""
        if direct_path:
            # :cd <path> — set directly if it exists
            p = Path(direct_path).expanduser().resolve()
            if p.is_dir():
                self.config_dir = str(p)
                self.save_data()
                self.popup_message = f"config dir → {self.config_dir}"
            else:
                self.popup_message = f"not a directory: {direct_path}"
        else:
            # :cd — interactive picker in dir-select mode
            picker = FilePicker(self.config_dir, self.colors)
            new_dir = picker.run(stdscr, pick_dir=True)
            if new_dir:
                self.config_dir = new_dir
                self.save_data()
                self.popup_message = f"config dir → {self.config_dir}"

    # ── drawing ───────────────────────────────────────────────────────────────

    def draw(self, stdscr):
        height, width = stdscr.getmaxyx()
        stdscr.clear()
        n = self.colors.get("normal", 0)
        g = self.colors.get("group", 0)

        stdscr.addstr(0, 1, "confy", g)
        if self.device_name:
            device_text = f"  [remote: {self.device_name}]"
            err = self.colors.get("error", g)
            try:
                stdscr.addstr(0, 6, device_text, err)
            except:
                pass
        last_text = f"previous: {{{Path(self.last_opened).name if self.last_opened else 'none'}}}"
        stdscr.addstr(1, 1, last_text, n)

        sort_text = f"sort: {self.sort_mode} ({self.sort_order})"
        config_text = f"config dir: {self.config_dir}"
        try:
            stdscr.addstr(1, width - len(config_text) - len(sort_text) - 5, sort_text, n)
            stdscr.addstr(1, width - len(config_text) - 2, config_text, n)
        except:
            pass
        stdscr.addstr(2, 0, "═" * (width - 1), n)

        # when preview is on, split the screen: list on the left, preview on the right,
        # separated by a vertical divider. list keeps a sane minimum width so it
        # doesn't get crushed on narrow terminals.
        preview_on = self.preview_enabled and width >= 60
        if preview_on:
            list_width = max(30, width // 2 - 1)
            divider_x = list_width + 1
        else:
            list_width = width

        ipp = self.items_per_page(stdscr)
        total_pages = max(1, (len(self.flat_view) + ipp - 1) // ipp)

        # clamp page if terminal was resized
        if self.page >= total_pages:
            self.page = max(0, total_pages - 1)

        start_idx = self.page * ipp
        end_idx = min(start_idx + ipp, len(self.flat_view))

        for i in range(start_idx, end_idx):
            y = 4 + (i - start_idx)
            item = self.flat_view[i]

            if item[0] == 'group':
                group_name = item[1]
                collapsed = "▶" if group_name in self.collapsed_groups else "▼"
                file_count = len(self.groups[group_name])
                line = f"{collapsed} {group_name}/ ({file_count} files)"
                attr = self.colors.get("highlight", curses.A_REVERSE) if i == self.selected else g
                if i == self.selected:
                    line += " <"
                try:
                    stdscr.addstr(y, 1, line[:list_width-2], attr)
                except:
                    pass

            elif item[0] == 'file':
                filepath = item[1]
                filename = Path(filepath).name
                directory = str(Path(filepath).parent)
                mtime, size = self.get_file_info(filepath)
                exists = os.path.exists(self.resolve_local_path(filepath))

                if preview_on:
                    # narrower layout, just filename + directory, no mtime/size (not enough room)
                    col_width = max(8, (list_width - 6) // 2)
                    line = f"  {filename[:col_width]:<{col_width}} | {directory[:col_width]:<{col_width}}"
                else:
                    col_width = max(10, (width - 60) // 2)
                    line = f"  {filename[:col_width]:<{col_width}} | {directory[:col_width]:<{col_width}} | {mtime} | {size}"

                if i == self.selected:
                    line += " <"
                    attr = self.colors.get("highlight", curses.A_REVERSE)
                elif not exists:
                    attr = self.colors.get("error", 0)
                else:
                    attr = n

                try:
                    stdscr.addstr(y, 1, line[:list_width-2], attr)
                except:
                    pass

        # preview pane
        if preview_on:
            try:
                for y in range(3, height - 2):
                    stdscr.addstr(y, divider_x, "│", n)
            except:
                pass

            preview_x = divider_x + 2
            preview_w = width - preview_x - 1
            preview_h = height - 5  # rows 4..height-2

            selected_item = self.flat_view[self.selected] if self.flat_view and self.selected < len(self.flat_view) else None

            if selected_item is None:
                header = "(nothing selected)"
                body_lines = []
            elif selected_item[0] == 'group':
                header = f"{selected_item[1]}/"
                file_count = len(self.groups[selected_item[1]])
                body_lines = [f"{file_count} file(s) in this group", "", "select a file to preview it"]
            else:
                filepath = selected_item[1]
                header = Path(filepath).name
                body_lines = self.get_preview_lines(filepath, preview_h - 2, preview_w)

            try:
                stdscr.addstr(3, preview_x, header[:preview_w], g)
                for j, line in enumerate(body_lines[:preview_h - 1]):
                    stdscr.addstr(5 + j, preview_x, line[:preview_w], n)
            except:
                pass

        bottom_y = height - 2
        try:
            stdscr.addstr(bottom_y, 0, "═" * (width - 1), n)
        except:
            pass

        if self.command_mode:
            page_text = f"page {self.page + 1}/{total_pages} ▌ :{self.command_buffer}"
        elif self.search_mode:
            page_text = f"page {self.page + 1}/{total_pages} ▌ /{self.search_buffer}"
        else:
            preview_hint = " ▌ p: preview on" if preview_on else " ▌ p: preview off"
            page_text = f"page {self.page + 1}/{total_pages}{preview_hint}"

        try:
            stdscr.addstr(bottom_y + 1, 1, page_text[:width-2], n)
        except:
            pass

        # popup message
        if self.popup_message:
            msg = f" {self.popup_message} "
            px = max(0, width // 2 - len(msg) // 2)
            try:
                stdscr.addstr(height // 2, px, msg, self.colors.get("highlight", curses.A_REVERSE))
            except:
                pass

        stdscr.refresh()

    # ── command handling ──────────────────────────────────────────────────────

    def handle_command(self, stdscr):
        cmd = self.command_buffer.strip()
        parts = cmd.split(maxsplit=1)

        if cmd == "q":
            return False
        elif cmd == "ac":
            self.add_config(stdscr)
        elif parts[0] == "ac" and len(parts) == 2:
            self.add_config(stdscr, parts[1])
        elif cmd == "rm":
            self.remove_config()
        elif parts[0] == "ag" and len(parts) == 2:
            self.add_group(parts[1])
        elif parts[0] == "rg" and len(parts) == 2:
            self.remove_group(parts[1])
        elif parts[0] == "mg" and len(parts) == 2:
            self.move_to_group(parts[1])
        elif cmd in ("h", "help"):
            show_tutorial(stdscr, self.colors, lambda: self.draw(stdscr))
        elif cmd == "l":
            if self.last_opened and os.path.exists(self.last_opened):
                self.open_file(self.last_opened)
        elif cmd == "cd":
            self.change_config_dir(stdscr)
        elif cmd == "cd reset":
            self.config_dir = str(Path.home() / ".config")
            self.save_data()
            self.popup_message = f"config dir reset to {self.config_dir}"
        elif parts[0] == "cd" and len(parts) == 2:
            self.change_config_dir(stdscr, direct_path=parts[1])
        elif parts[0] == "sort" and len(parts) == 2:
            if parts[1] in ["name", "date", "size"]:
                self.sort_mode = parts[1]
                self.save_data()
                self.rebuild_flat_view()
        elif cmd == "reverse":
            self.sort_order = "desc" if self.sort_order == "asc" else "asc"
            self.save_data()
            self.rebuild_flat_view()
        elif cmd == "rb":
            # rollback selected file
            if self.flat_view and self.selected < len(self.flat_view):
                item = self.flat_view[self.selected]
                if item[0] == 'file':
                    filepath = item[1]
                    if self.confirm_popup(stdscr, f"rollback {Path(filepath).name}?"):
                        ok, msg = self.rollback(filepath)
                        self.popup_message = msg
                    else:
                        self.popup_message = "rollback cancelled"
                else:
                    self.popup_message = "select a file first"
            else:
                self.popup_message = "nothing selected"
        elif cmd == "theme":
            names = ", ".join(THEMES.keys())
            self.popup_message = f"themes: {names}"
        elif parts[0] == "theme" and len(parts) == 2:
            self.set_theme(parts[1])
        elif parts[0] in ("device", "ssh") and len(parts) == 2:
            self.switch_device(parts[1], stdscr)
        elif cmd in ("device", "ssh"):
            if self.device_name:
                self.popup_message = f"currently on device: {self.device_name}"
            else:
                self.popup_message = "usage: :device <alias-or-user@host>, or :device local"
        elif cmd == "su":
            if self.flat_view and self.selected < len(self.flat_view):
                item = self.flat_view[self.selected]
                if item[0] == 'file':
                    self.open_file_elevated(item[1])
                else:
                    self.popup_message = "select a file first, not a group"
            else:
                self.popup_message = "nothing selected"

        self.command_buffer = ""
        self.command_mode = False
        return True

    # ── main loop ─────────────────────────────────────────────────────────────

    def run(self, stdscr):
        self.colors = init_colors(self.settings.get('colors', DEFAULT_SETTINGS['colors']))
        curses.curs_set(0)
        # no timeout — blocking getch() prevents constant redraws and flicker

        # first-startup tutorial
        if self.show_tutorial:
            show_tutorial(stdscr, self.colors, lambda: self.draw(stdscr))
            self.settings['first_startup'] = False
            self.save_data()

        self.draw(stdscr)

        while True:
            # show popup: draw to reveal it, wait for keypress, clear and redraw
            if self.popup_message:
                self.draw(stdscr)
                stdscr.getch()
                self.popup_message = None
                self.draw(stdscr)
                continue

            try:
                key = stdscr.getch()
            except:
                continue

            if key == -1:
                continue  # spurious wakeup, skip redraw

            ipp = self.items_per_page(stdscr)
            total_pages = max(1, (len(self.flat_view) + ipp - 1) // ipp)

            if self.command_mode:
                if key == ord('\n'):
                    if not self.handle_command(stdscr):
                        break
                elif key == 27:
                    self.command_mode = False
                    self.command_buffer = ""
                elif key in (curses.KEY_BACKSPACE, 127, 8):
                    self.command_buffer = self.command_buffer[:-1]
                elif 32 <= key <= 126:
                    self.command_buffer += chr(key)

            elif self.search_mode:
                if key == ord('\n'):
                    self.search_mode = False
                    self.selected = 0
                    self.page = 0
                elif key == 27:
                    self.search_mode = False
                    self.search_buffer = ""
                    self.rebuild_flat_view()
                    self.selected = 0
                    self.page = 0
                elif key in (curses.KEY_BACKSPACE, 127, 8):
                    self.search_buffer = self.search_buffer[:-1]
                    self.rebuild_flat_view()
                    self.selected = 0
                    self.page = 0
                elif 32 <= key <= 126:
                    self.search_buffer += chr(key)
                    self.rebuild_flat_view()
                    self.selected = 0
                    self.page = 0

            else:
                if key == ord(':'):
                    self.command_mode = True
                    self.command_buffer = ""
                elif key == ord('/'):
                    self.search_mode = True
                    self.search_buffer = ""
                elif key in (ord('j'), curses.KEY_DOWN):
                    if self.selected < len(self.flat_view) - 1:
                        self.selected += 1
                        if self.selected >= (self.page + 1) * ipp:
                            self.page += 1
                elif key in (ord('k'), curses.KEY_UP):
                    if self.selected > 0:
                        self.selected -= 1
                        if self.selected < self.page * ipp:
                            self.page -= 1
                elif key == ord('\n'):
                    if self.flat_view and self.selected < len(self.flat_view):
                        item = self.flat_view[self.selected]
                        if item[0] == 'file':
                            self.open_file(item[1])
                        elif item[0] == 'group':
                            self.toggle_group()
                elif key == ord(' '):
                    self.toggle_group()
                elif key == ord('p'):
                    self.preview_enabled = not self.preview_enabled
                    self.save_data()
                elif key == ord('q'):
                    break

            self.draw(stdscr)


def main():
    app = Confy()
    try:
        curses.wrapper(app.run)
    finally:
        # always clean up a mounted device on exit, even on crash/ctrl-c
        if app.device_mountpoint is not None:
            DeviceManager.unmount(app.device_mountpoint)

if __name__ == "__main__":
    main()

