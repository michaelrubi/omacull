# Omacull

A fast, keyboard-first photo culler for [Omarchy](https://omarchy.org).
Open a folder of raws, step through them without waiting, mark picks,
rejects and stars, and hand the folder to darktable.

> Omacull is an independent project. It is not made by or affiliated with
> Omarchy.

It's the first stage of one workflow: cull in Omacull, base edit in
darktable, retouch in [Omapix](https://github.com/michaelrubi/omapix),
export.

**Status:** early. The basic culler is in (M1): `omacull <folder>`, arrows
to step, P/X/U and 0-5 to mark, Ctrl+Z to undo. So is inspection (M2): Z
for 100%, H histogram, J clipping, S focus peaking, F the focus point, I
the shooting settings. See [docs/DESIGN.md](docs/DESIGN.md)
for what it will and won't do and [docs/ROADMAP.md](docs/ROADMAP.md) for
the plan.

## License

GPL-3.0-or-later.
