{RELEASE} - read me first
Version {VERSION} (build {HASH})

WHAT THIS IS
A preview of ARTY, a manga and illustration drawing program made to run well on
modest computers. It is a test build: it works, but it is not finished. Thank you
for trying it and telling us what went wrong.

WHAT YOU NEED (the lowest computer it is designed for)
  - Windows 10 (22H2, 64-bit) or Windows 11
  - 4 GB of RAM (8 GB is more comfortable)
  - A processor with 2 cores / 4 threads, for example Intel Core i3-6100U or a
    4-core Celeron / Pentium / Atom (N4120, N100 and similar)
  - Graphics: Intel HD 520 / UHD 600, AMD Vega 3, or anything newer. The graphics
    card built into the processor is fine.
  - A hard disk or SSD (an SSD starts faster). A screen of 1366 x 768 or larger.
  - A mouse, or a pen tablet (a cheap one is fine)
Nothing needs to be installed. You do NOT need to install Visual C++ or anything else.

HOW TO START
 1. Unzip the whole zip file into a folder (right-click, Extract All). Do not run
    arty.exe from inside the zip.
 2. Double-click arty.exe.
 3. Windows may say "Windows protected your PC" because the program is not signed
    yet. Click "More info", then "Run anyway".
To remove ARTY, delete the folder. (Settings are kept in %APPDATA%\ARTY and
recovery files and logs in %LOCALAPPDATA%\ARTY; delete those too if you want.)

YOUR WORK IS PROTECTED
ARTY saves a recovery copy of the page you are working on about every minute
(File > Autosave). If the program closes by itself or the computer loses power, start
ARTY again: the start screen has "Open / recover", which lists the unsaved work.
Recovery files live in %LOCALAPPDATA%\ARTY\recovery. Please still use File > Save
for anything you care about.

IF SOMETHING GOES WRONG
ARTY writes a log file every time it runs. They are in
  %LOCALAPPDATA%\ARTY\logs
  arty-DATE-TIME.log   one per run, the newest 5 are kept
  crash-DATE-TIME.txt  written only if ARTY crashes
 1. In ARTY choose Help > Report a problem. Your browser opens our feedback form and
    a window opens on the log folder.
 2. In the form say what you were doing and what happened.
 3. Attach the newest .log file from the folder (and the crash-....txt file if there is one).
Or open the folder yourself with Help > Open log folder.
Nothing is ever sent automatically; you choose what to attach. The log has the program
version, your Windows version, the computer's memory, processor cores and graphics
card name, and may contain the names of files you opened.
If ARTY closed unexpectedly last time, it tells you when you start it again.

KNOWN PROBLEMS (we know about these, no need to report them)
  - Filling "to darkest" on a page with many layers (around 35) can freeze ARTY for
    about a second.
  - After you undo a selection move or transform, the moving dotted outline of the
    selection may look old for a moment.
  - The corner resize cursors point the wrong way on a long, narrow selection box.
  - A rectangle or lasso selection can be committed by mistake if you press Space
    while dragging.
  - Thai text: the vowel "ำ" can be split from its letter when a line wraps, and the
    text cursor can be in the wrong place after "ำ".
  - It is slower on the lowest computers; very large pages with many layers need
    more RAM than 4 GB.

LICENCE
Free for personal testing only. Do not redistribute. All rights reserved by the owner.
See LICENSE-PREVIEW.txt. Open-source parts used inside ARTY are listed in
THIRD_PARTY_NOTICES.txt (also under Help > About ARTY > Licences).
