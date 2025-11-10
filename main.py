#!/usr/bin/env python3

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
        self.files = []
        self.selected = 0
        self.page = 0
        self.items_per_page = 10
        self.command_mode = False
        self.command_buffer = ""
        self.last_opened = None
        self.config_dir = str(Path.home() / ".config")
        self.load_data()

    def load_data(self):
        if TRACKED_FILE.exists():
            with open(TRACKED_FILE, 'r') as f:
                data = json.load(f)
                self.files = data.get('files', [])
                self.last_opened = data.get('last_opened')

    def save_data(self):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        with open(TRACKED_FILE, 'w') as f:
            json.dump({
                'files': self.files,
                'last_opened': self.last_opened
            }, f, indent=2)

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

    def add_config(self):
        curses.endwin()
        try:
            result = subprocess.run(['ranger', '--choosefile=/tmp/confy_pick', self.config_dir])
            if os.path.exists('/tmp/confy_pick'):
                with open('/tmp/confy_pick', 'r') as f:
                    filepath = f.read().strip()
                if filepath and filepath not in self.files:
                    self.files.append(filepath)
                    self.save_data()
                os.remove('/tmp/confy_pick')
        except Exception as e:
            pass
        curses.doupdate()

    def remove_config(self):
        if self.files and 0 <= self.selected < len(self.files):
            del self.files[self.selected]
            if self.selected >= len(self.files) and self.files:
                self.selected = len(self.files) - 1
            self.save_data()

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
        config_text = f"config dir is {self.config_dir}"
        stdscr.addstr(1, width - len(config_text) - 2, config_text)
        stdscr.addstr(2, 0, "═" * width)

        # file list
        start_idx = self.page * self.items_per_page
        end_idx = min(start_idx + self.items_per_page, len(self.files))
        
        for i in range(start_idx, end_idx):
            y = 4 + (i - start_idx)
            filepath = self.files[i]
            filename = Path(filepath).name
            directory = str(Path(filepath).parent)
            mtime, size = self.get_file_info(filepath)
            
            line = f"{filename:<20} | {directory:<30} | {mtime:<16} | {size:<10}"
            if i == self.selected:
                line = f"{line} <"
                stdscr.attron(curses.A_REVERSE)
            stdscr.addstr(y, 1, line[:width-2])
            if i == self.selected:
                stdscr.attroff(curses.A_REVERSE)

        # bottom bar
        total_pages = (len(self.files) + self.items_per_page - 1) // self.items_per_page
        if total_pages == 0:
            total_pages = 1
        bottom_y = height - 2
        stdscr.addstr(bottom_y, 0, "═" * width)
        if self.command_mode:
            page_text = f"page {self.page + 1}/{total_pages} ▌ :{self.command_buffer}"
        else:
            page_text = f"page {self.page + 1}/{total_pages} ▌"
        stdscr.addstr(bottom_y + 1, 1, page_text[:width-2])

        stdscr.refresh()

    def handle_command(self):
        cmd = self.command_buffer.strip()
        if cmd == "q":
            return False
        elif cmd == "ac":
            self.add_config()
        elif cmd == "rm":
            self.remove_config()
        elif cmd == "l":
            if self.last_opened and os.path.exists(self.last_opened):
                self.open_file(self.last_opened)
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
            else:
                if key == ord(':'):
                    self.command_mode = True
                    self.command_buffer = ""
                elif key in (ord('j'), curses.KEY_DOWN):
                    if self.selected < len(self.files) - 1:
                        self.selected += 1
                        if self.selected >= (self.page + 1) * self.items_per_page:
                            self.page += 1
                elif key in (ord('k'), curses.KEY_UP):
                    if self.selected > 0:
                        self.selected -= 1
                        if self.selected < self.page * self.items_per_page:
                            self.page -= 1
                elif key == ord('\n'):
                    if self.files and 0 <= self.selected < len(self.files):
                        self.open_file(self.files[self.selected])
                elif key == ord('q'):
                    break

def main():
    app = Confy()
    curses.wrapper(app.run)

if __name__ == "__main__":
    main()
