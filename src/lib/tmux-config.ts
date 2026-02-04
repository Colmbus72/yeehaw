import { writeFileSync } from 'fs';
import { join } from 'path';
import { YEEHAW_DIR } from './paths.js';

export const TMUX_CONFIG_PATH = join(YEEHAW_DIR, 'tmux.conf');

export function generateTmuxConfig(): string {
  return `
# Yeehaw tmux configuration
# Auto-generated - do not edit manually

# Scrollback and mouse support
set -g mouse on
set -g history-limit 50000

# macOS clipboard support
# Enable copying to system clipboard when selecting with mouse
set -g set-clipboard on
bind-key -T copy-mode MouseDragEnd1Pane send-keys -X copy-pipe-and-cancel "pbcopy"
bind-key -T copy-mode-vi MouseDragEnd1Pane send-keys -X copy-pipe-and-cancel "pbcopy"
# Also support keyboard-based copy (Enter key in copy mode)
bind-key -T copy-mode Enter send-keys -X copy-pipe-and-cancel "pbcopy"
bind-key -T copy-mode-vi Enter send-keys -X copy-pipe-and-cancel "pbcopy"
# Use y to yank in vi mode
bind-key -T copy-mode-vi y send-keys -X copy-pipe-and-cancel "pbcopy"

# Yeehaw keybindings
bind-key -n C-y select-window -t :0    # Return to dashboard
bind-key -n C-h previous-window        # Go left one window
bind-key -n C-l next-window            # Go right one window

# Status bar styling (Yeehaw brand colors)
set -g status-style "bg=#b8860b,fg=#1a1a1a"
set -g status-left "#[bold] YEEHAW "
set -g status-left-length 20
set -g status-right " Ctrl-y: dashboard "
set -g status-right-length 30

# Window status format
set -g window-status-format " #I:#W "
set -g window-status-current-format "#[bg=#daa520,fg=#1a1a1a,bold] #I:#W "

# Pane border styling
set -g pane-border-style "fg=#b8860b"
set -g pane-active-border-style "fg=#daa520"

# Message styling
set -g message-style "bg=#b8860b,fg=#1a1a1a"
`.trim();
}

export function writeTmuxConfig(): string {
  const content = generateTmuxConfig();
  writeFileSync(TMUX_CONFIG_PATH, content);
  return TMUX_CONFIG_PATH;
}
