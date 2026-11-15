# Tasks: spike_gpui_surface

- [ ] spike: cold build of gpui-kit 0.9; record wall clock and binary size
- [ ] spike: TextView streams a tape at full token rate, headless
- [ ] spike: closed block keeps its layout; only the tail block re-parses
- [ ] spike: frame time via gpui-fps, storage writer and checkpoint running alongside
- [ ] spike: open block renders monospace; table grids lay out at close
- [ ] spike: text field, IME composition, and paste under load
- [ ] spike: AccessKit tree present in a headless window
- [ ] spike: a stream from a tokio-owned HTTP leaf, polled on gpui's executor: bytes arrive, cancel-by-drop stops it, and the main-thread CPU of a real TLS + gzip response is measured — `adopt_async_core`'s two-scheduler topology rests on this
- [ ] spike: a push queue at full token rate: a bounded lossy queue coalesces, an unbounded one grows the surface's memory, both inside a frame budget this spike sets, from the display's own cadence (~16.7 ms at 60 fps) — the `live output under load` gate row in `AGENTS.md` cites this spike for the number, so the number lives here — the surface's event-stream clause is written from this
- [ ] spike: record the fallback B verdict against closed-block immutability
- [ ] spike: write results into proposal.md
- [ ] spec-review (reviewer ticks)
