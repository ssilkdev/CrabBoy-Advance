
eframe/src/native/run.rs (battery): check_redraw_requests() set
ControlFlow::Poll for every due repaint, even when the window was gone
(Android after Home / screen off), and never reset it, so the event loop
spun at 100% of a core in the background. It now polls only when a redraw
was actually requested and otherwise waits.
