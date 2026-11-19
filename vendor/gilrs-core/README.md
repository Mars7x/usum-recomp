GilRs Core
==========

[![pipeline status](https://gitlab.com/gilrs-project/gilrs/badges/master/pipeline.svg)](https://gitlab.com/gilrs-project/gilrs-core/commits/master)
[![Minimum rustc version](https://img.shields.io/badge/rustc-1.64.0+-yellow.svg)](https://gitlab.com/gilrs-project/gilrs)

This library is minimal event-based abstraction for working with gamepads. If
you are looking for something more high level, take a look at `gilrs` crate.

Platform specific notes
======================

Linux
-----

On Linux, GilRs read (and write, in case of force feedback) directly from
appropriate `/dev/input/event*` file. This means that user has to have read and
write access to this file. On most distros it shouldn’t be a problem, but if
it is, you will have to create udev rule.

This vendored version discovers Linux gamepads using inotify. See [USUM.md](USUM.md).

## License

This copy is distributed under the Apache License 2.0. See LICENSE-APACHE.
