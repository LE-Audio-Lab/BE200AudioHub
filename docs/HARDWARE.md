# Experimental BE200 rig

- Controller: Intel BE200NGW Bluetooth USB, observed VID:PID **8087:0036**.
- E-key → NVMe M-key converter with separately exposed USB2 D+/D−.
- M-key breakout supplies external 3.3 V and GND, verified by continuity on the user's actual adapter.
- USB-A-powered adjustable 3.3 V supply; common USB host ground previously verified.
- BE200 data pair and MT7925 data pair occupy separate ports of one modified front USB2 9-pin connector.
- Reference USBPcap interface: USBPcap1; USB Root Hub reports BE200 at physical port 14, MT7925 at port 11 (hub topology, not device addresses).
- Parent build: Windows 11 Pro (historically 26200); exact present build must be read from current diagnostics.

**Safety:** The exposed M.2 breakout and hand-repacked header are experimental hardware. R1.1/R1.2 do not initiate resets, power cycling, driver changes or interface claims. Do not hot-rewire or confuse M-key numbers with E-key numbers.
