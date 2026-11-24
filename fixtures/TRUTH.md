# Fixture ground truth (rekordbox 6.8.5, 96 dpi, window content 2560x1410)

PNG files are 2576x1426. Only the top-left 2560x1410 is window content; the rest is padding.

All fixtures:
- waveform band y=149..287
- row 1 (deck 1) y=151..215, row 2 (deck 2) y=220..285
- playhead x = 1280

| file | state | waveform colour | deck1 Bars text | deck2 Bars text | master | notes |
|---|---|---|---|---|---|---|
| 3band-playing-a.png | both playing | 3Band | 85.3 | 69.1 | deck 1 | row1 red marker centres 156, 327, 498, 668, 839, 1009, 1180, 1351; row2 126, 316, 506, 697, 887, 1078, 1268, 1459 |
| 3band-playing-b.png | both playing, 0.75 s after -a | 3Band | ? | ? | deck 1 | row1 red starts 49, 220; row2 208, 398 |
| 3band-paused.png | both paused | 3Band | 45.1 | 113.1 | deck 2 | playhead colour 707070 |
| blue-paused.png | both paused | Blue | 45.1 | 113.1 | ? | "113.1Bars" covers one top red triangle near x=1238 |
| rgb-paused.png | both paused | RGB | 45.1 | 113.1 | ? | waveform body has pure red pixels |
| session-000/020.png | paused | RGB | ? | ? | deck 2 | |
| session-036..055.png | deck 1 playing, tempo changed | RGB | ? | ? | deck 2 | deck 1 bar spacing 171 -> 172 px |

Measured colours:
- bar triangle: ff0000
- bar line: ffffff
- beat triangle: 7c7c7c
- beat line: 4c4c4c
- playhead: e0e0e0 playing, 707070 paused
- Bars text: 007de1 (anti-aliased, R<=10, G/B 0.52-0.60)
- MASTER label active: ff8c19 (deck 1 x~1160-1225, y 312-332; deck 2 x~2490-2555)
- BPM header text: grey >= 110, deck 1 x=218.., deck 2 x=1552.., y=316..324

"?" = not recorded. Derive it from the image and state how you derived it.
