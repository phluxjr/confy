#!/usr/bin/env python3
# confy - a config manager for linux/unix systems
# Copyright (C) 2025-2026 phluxjr
# Licensed under GPL-3.0-or-later
import curses
import json
import os
import subprocess
from pathlib import Path
from datetime import datetime

CONFIG_DIR = Path.home() / ".config" / "confy"
TRACKED_FILE = CONFIG_DIR / "tracked.json"

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
        self.sort_mode = "name"  # name, date, size
        self.sort_order = 'asc'  # asc or desc
        self.load_data()
        self.rebuild_flat_view()

    def load_data(self):
        if TRACKED_FILE.exists():
            with open(TRACKED_FILE, 'r') as f:
                data = json.load(f)
                if 'files' in data:
                    self.groups = {"ungrouped": data['files']}
                else:
                    self.groups = data.get('groups', {"ungrouped": []})
                self.last_opened = data.get('last_opened')
                self.collapsed_groups = set(data.get('collapsed_groups', []))
                self.sort_mode = data.get('sort_mode', 'name')
                self.sort_order = data.get('sort_order', 'asc')

    def save_data(self):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        with open(TRACKED_FILE, 'w') as f:
            json.dump({
                'groups': self.groups,
                'last_opened': self.last_opened,
                'collapsed_groups': list(self.collapsed_groups),
                'sort_mode': self.sort_mode,
                'sort_order': self.sort_order
            }, f, indent=2)

    def sort_files(self, files):
        """sort files based on current sort mode"""
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
        """rebuild flattened view for navigation"""
        self.flat_view = []
        
        # filter by search if active
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

    def add_config(self, group_name="ungrouped"):
        curses.endwin()
        try:
            result = subprocess.run(['ranger', '--choosefile=/tmp/confy_pick', self.config_dir])
            if os.path.exists('/tmp/confy_pick'):
                with open('/tmp/confy_pick', 'r') as f:
                    filepath = f.read().strip()
                if filepath:
                    for grp_files in self.groups.values():
                        if filepath in grp_files:
                            os.remove('/tmp/confy_pick')
                            curses.doupdate()
                            return
                    if group_name not in self.groups:
                        self.groups[group_name] = []
                    self.groups[group_name].append(filepath)
                    self.save_data()
                    self.rebuild_flat_view()
                os.remove('/tmp/confy_pick')
        except Exception as e:
            pass
        curses.doupdate()

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

    def change_config_dir(self):
        curses.endwin()
        try:
            result = subprocess.run(['ranger', '--choosedir=/tmp/confy_dir'])
            if os.path.exists('/tmp/confy_dir'):
                with open('/tmp/confy_dir', 'r') as f:
                    new_dir = f.read().strip()
                if new_dir and os.path.isdir(new_dir):
                    self.config_dir = new_dir
                os.remove('/tmp/confy_dir')
        except Exception as e:
            pass
        curses.doupdate()

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
        editor = os.environ.get('EDITOR', 'nano')
        curses.endwin()
        try:
            subprocess.run([editor, filepath])
        except FileNotFoundError:
            input(f"error: editor '{editor}' not found. press enter to continue...")
        except Exception as e:
            input(f"error opening file: {e}. press enter to continue...")
        curses.doupdate()
        self.last_opened = filepath
        self.save_data()

    def draw(self, stdscr):
        height, width = stdscr.getmaxyx()
        stdscr.clear()

        # top bar
        stdscr.addstr(0, 1, "confy")
        last_text = f"previous: {{{Path(self.last_opened).name if self.last_opened else 'none'}}}"
        stdscr.addstr(1, 1, last_text)
        
        # show sort mode and config dir
        sort_text = f"sort: {self.sort_mode} ({self.sort_order})"
        config_text = f"config dir: {self.config_dir}"
        stdscr.addstr(1, width - len(config_text) - len(sort_text) - 5, sort_text)
        stdscr.addstr(1, width - len(config_text) - 2, config_text)
        stdscr.addstr(2, 0, "═" * width)

        # file list with groups
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
                
                if i == self.selected:
                    line = f"{line} <"
                    stdscr.attron(curses.A_REVERSE | curses.A_BOLD)
                else:
                    stdscr.attron(curses.A_BOLD)
                
                stdscr.addstr(y, 1, line[:width-2])
                stdscr.attroff(curses.A_BOLD)
                if i == self.selected:
                    stdscr.attroff(curses.A_REVERSE)
                    
            elif item[0] == 'file':
                filepath = item[1]
                filename = Path(filepath).name
                directory = str(Path(filepath).parent)
                mtime, size = self.get_file_info(filepath)
                
                # dynamically size columns based on terminal width
                col_width = max(10, (width - 60) // 2)
                line = f"  {filename[:col_width]:<{col_width}} | {directory[:col_width]:<{col_width}} | {mtime} | {size}"
                if i == self.selected:
                    line = f"{line} <"
                    stdscr.attron(curses.A_REVERSE)
                stdscr.addstr(y, 1, line[:width-2])
                if i == self.selected:
                    stdscr.attroff(curses.A_REVERSE)

        # bottom bar
        total_pages = (len(self.flat_view) + self.items_per_page - 1) // self.items_per_page
        if total_pages == 0:
            total_pages = 1
        bottom_y = height - 2
        stdscr.addstr(bottom_y, 0, "═" * width)
        
        if self.command_mode:
            page_text = f"page {self.page + 1}/{total_pages} ▌ :{self.command_buffer}"
        elif self.search_mode:
            page_text = f"page {self.page + 1}/{total_pages} ▌ /{self.search_buffer}"
        else:
            page_text = f"page {self.page + 1}/{total_pages} ▌"
        stdscr.addstr(bottom_y + 1, 1, page_text[:width-2])

        stdscr.refresh()

    def handle_command(self):
        cmd = self.command_buffer.strip()
        parts = cmd.split(maxsplit=1)
        
        if cmd == "q":
            return False
        elif cmd == "ac":
            self.add_config()
        elif parts[0] == "ac" and len(parts) == 2:
            self.add_config(parts[1])
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
            self.change_config_dir()
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
        
        self.command_buffer = ""
        self.command_mode = False
        return True

    def run(self, stdscr):
        curses.curs_set(0)
        stdscr.timeout(100)

        while True:
            self.draw(stdscr)
            
            try:
                key = stdscr.getch()
            except:
                continue

            if self.command_mode:
                if key == ord('\n'):
                    if not self.handle_command():
                        break
                elif key == 27:  # ESC
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
                elif key == 27:  # ESC
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
