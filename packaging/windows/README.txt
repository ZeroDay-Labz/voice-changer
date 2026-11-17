Voice Changer for Windows (preview)
===================================

Start voice-changer.exe. Nothing is installed and nothing on your computer
is changed: to remove Voice Changer, delete this folder.

How it works
------------
Pick your microphone and an output at the top right of the window. Your
changed voice plays on that output: headphones, speakers, a mixer or an
audio interface. Voice Changer does not create a virtual microphone and
does not touch your sound settings.

To get the voice into a program that only listens to a microphone (a call,
a recording program), you need something that turns an output into an
input: a mixer or audio interface with a loopback, or a virtual cable you
installed yourself. If you have one, it simply appears in the output list.

First start
-----------
Windows may show "Windows protected your PC" because the program is not
signed. Choose "More info", then "Run anyway".

If no sound arrives: Settings > Privacy & security > Microphone, and allow
desktop apps to use the microphone.

Keep these files together
-------------------------
voice-changer.exe    the program
onnxruntime.dll, onnxruntime_providers_shared.dll, DirectML.dll
                     the engine for AI voices; the program looks for them
                     next to itself
vc-plugin.clap       the same voice changer as a CLAP plugin; copy it to
                     C:\Program Files\Common Files\CLAP to use it in a DAW
licenses\            licences of Voice Changer and of the libraries above

Where your things are kept
--------------------------
Presets and settings:   %APPDATA%\echo\voice-changer\config
AI voices and models:   %APPDATA%\echo\voice-changer\data
(Settings > Log > "Open data folder" opens it.)

From a terminal
---------------
voice-changer.exe --help          every option
voice-changer.exe doctor          checks this computer and says what is wrong
voice-changer.exe toggle          switches the running program on or off
voice-changer.exe mute            mutes or unmutes it

This is a preview
-----------------
The Windows version is built and tested automatically, but it has not been
run on real sound hardware by its author yet. AI voices run on the
processor; the graphics card (DirectML) can be chosen under Home > AI voice
> More settings > Runs on, and is experimental. Please report what you
find: https://github.com/ZeroDay-Labz/voice-changer/issues
