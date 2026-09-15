# Game title bar

Settings → Appearance → Game window → Title bar offers **Default**, **Compact**
and **Hidden**. The choice applies to the next game window you open.

This setting applies only to Wayland game windows. The X11 runtime uses an
unrelated native window and is unaffected.

Hidden removes the title bar without requesting fullscreen, so a tiled window
keeps its tile. Entering and leaving fullscreen does not restore a hidden bar.
Default remains the default for new installations.

Window controls are absent in Hidden mode. Use your desktop's window shortcuts
to move or close the game, and the Cordial launcher to change this preference.
For direct `cordial-run` launches, `CORDIAL_TITLE_BAR=hidden` selects the same mode.
