#!/usr/bin/env python3
# confy - a config manager for linux/unix systems
# Copyright (C) 2025-2026 phluxjr
# Licensed under GPL-3.0-or-later
import curses
import json
import os
import gzip
import shutil
import subprocess
from pathlib import Path
from datetime import datetime

CONFIG_DIR = Path.home() / ".config" / "confy"
TRACKED_FILE = CONFIG_DIR / "tracked.json"  # legacy
CONFIG_FILE  = CONFIG_DIR / "config.json"

# ── default app config (user can override in config.json under "settings") ────

DEFAULT_SETTINGS = {
    "rollback": True,
    "colors": {
        "bg":        "default",   # terminal default or hex like "#1e1e2e"
        "fg":        "default",
        "highlight": "#cba6f7",   # catppuccin mauve as default lol
        "group":     "#89b4fa",   # catppuccin blue
        "border":    "default",
    }
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

    def run(self, stdscr):
        self.load_entries()
        curses.curs_set(0)

        while True:
            stdscr.clear()
            h, w = stdscr.getmaxyx()

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
                    return str(entry)
            elif key in (curses.KEY_BACKSPACE, 127, 8):
                self.cwd = self.cwd.parent
                self.selected = 0
                self.scroll = 0
                self.load_entries()

# ── main app ──────────────────────────────────────────────────────────────────

class Confy:
    def __init__(self):
        self.groups = {"ungrouped": []}
        self.selected = 0
        self.page = 0
        self.items_per_page = 10
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
        self.migrate_if_needed()
        self.load_data()
        self.rebuild_flat_view()

    def migrate_if_needed(self):
        """migrate tracked.json -> config.json if needed"""
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        if not CONFIG_FILE.exists() and TRACKED_FILE.exists():
            shutil.copy(TRACKED_FILE, CONFIG_FILE)
            self.popup_message = "migrated tracked.json → config.json!"

    def load_data(self):
        if CONFIG_FILE.exists():
            with open(CONFIG_FILE, 'r') as f:
                data = json.load(f)
                if 'files' in data:
                    self.groups = {"ungrouped": data['files']}
                else:
                    self.groups = data.get('groups', {"ungrouped": []})
                self.last_opened = data.get('last_opened')
                self.collapsed_groups = set(data.get('collapsed_groups', []))
                self.sort_mode = data.get('sort_mode', 'name')
                self.sort_order = data.get('sort_order', 'asc')
                # load user settings, merging with defaults
                user_settings = data.get('settings', {})
                self.settings.update(user_settings)
                if 'colors' in user_settings:
                    self.settings['colors'] = {**DEFAULT_SETTINGS['colors'], **user_settings['colors']}

    def save_data(self):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        with open(CONFIG_FILE, 'w') as f:
            json.dump({
                'groups': self.groups,
                'last_opened': self.last_opened,
                'collapsed_groups': list(self.collapsed_groups),
                'sort_mode': self.sort_mode,
                'sort_order': self.sort_order,
                'settings': self.settings,
            }, f, indent=2)

    # ── rollback ──────────────────────────────────────────────────────────────

    def save_backup(self, filepath):
        """save compressed backup of filepath to /tmp"""
        if not self.settings.get('rollback', True):
            return
        try:
            bak_name = Path(filepath).name + ".confbak"
            bak_path = Path("/tmp") / bak_name
            with open(filepath, 'rb') as f_in:
                with gzip.open(bak_path, 'wb') as f_out:
                    shutil.copyfileobj(f_in, f_out)
        except Exception:
            pass

    def rollback(self, filepath):
        """restore backup for filepath, returns (success, message)"""
        bak_name = Path(filepath).name + ".confbak"
        bak_path = Path("/tmp") / bak_name
        if not bak_path.exists():
            return False, "no backup found in /tmp"
        try:
            with gzip.open(bak_path, 'rb') as f_in:
                with open(filepath, 'wb') as f_out:
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
            sorted_files = sorted(files, key=lambda f: os.path.getmtime(f) if os.path.exists(f) else 0)
        elif self.sort_mode == "size":
            sorted_files = sorted(files, key=lambda f: os.path.getsize(f) if os.path.exists(f) else 0)
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
        try:
            stat = os.stat(filepath)
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

    # ── file operations ───────────────────────────────────────────────────────

    def add_config(self, stdscr, group_name="ungrouped"):
        picker = FilePicker(self.config_dir, self.colors)
        filepath = picker.run(stdscr)
        if filepath:
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
        self.save_backup(filepath)
        editor = os.environ.get('EDITOR', 'nano')
        curses.endwin()
        try:
            subprocess.run([editor, filepath])
        except FileNotFoundError:
            input(f"error: editor '{editor}' not found. press enter...")
        except Exception as e:
            input(f"error opening file: {e}. press enter...")
        curses.doupdate()
        self.last_opened = filepath
        self.save_data()

    # ── drawing ───────────────────────────────────────────────────────────────

    def draw(self, stdscr):
        height, width = stdscr.getmaxyx()
        stdscr.clear()
        n = self.colors.get("normal", 0)
        g = self.colors.get("group", 0)

        stdscr.addstr(0, 1, "confy", g)
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

        start_idx = self.page * self.items_per_page
        end_idx = min(start_idx + self.items_per_page, len(self.flat_view))

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
                    stdscr.addstr(y, 1, line[:width-2], attr)
                except:
                    pass

            elif item[0] == 'file':
                filepath = item[1]
                filename = Path(filepath).name
                directory = str(Path(filepath).parent)
                mtime, size = self.get_file_info(filepath)
                exists = os.path.exists(filepath)

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
                    stdscr.addstr(y, 1, line[:width-2], attr)
                except:
                    pass

        total_pages = max(1, (len(self.flat_view) + self.items_per_page - 1) // self.items_per_page)
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
            page_text = f"page {self.page + 1}/{total_pages} ▌"

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
        elif cmd == "l":
            if self.last_opened and os.path.exists(self.last_opened):
                self.open_file(self.last_opened)
        elif cmd == "cd":
            picker = FilePicker(self.config_dir, self.colors)
            # pick a dir: just navigate until they quit, use cwd as result
            picker.run(stdscr)  # returns file, but cwd changes as they browse
        elif cmd == "cd reset":
            self.config_dir = str(Path.home() / ".config")
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

        self.command_buffer = ""
        self.command_mode = False
        return True

    # ── main loop ─────────────────────────────────────────────────────────────

    def run(self, stdscr):
        self.colors = init_colors(self.settings.get('colors', DEFAULT_SETTINGS['colors']))
        curses.curs_set(0)
        stdscr.timeout(100)

        while True:
            self.draw(stdscr)

            # clear popup after one frame
            if self.popup_message:
                stdscr.getch()
                self.popup_message = None
                continue

            try:
                key = stdscr.getch()
            except:
                continue

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
                        if self.selected >= (self.page + 1) * self.items_per_page:
                            self.page += 1
                elif key in (ord('k'), curses.KEY_UP):
                    if self.selected > 0:
                        self.selected -= 1
                        if self.selected < self.page * self.items_per_page:
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
                elif key == ord('q'):
                    break


def main():
    app = Confy()
    curses.wrapper(app.run)

if __name__ == "__main__":
    main()
