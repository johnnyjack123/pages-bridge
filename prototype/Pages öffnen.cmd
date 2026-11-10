@echo off
rem Drop one or more .pages files onto this file: each is converted to .docx and opened in WordCraft.
py -3 "%~dp0pages2docx.py" --open %*
if errorlevel 1 pause
