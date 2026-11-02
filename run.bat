@echo off
REM Starts the overlay with no console window. Quit with Ctrl+Shift+F8.
start "" pythonw "%~dp0aram_overlay.py" %*
