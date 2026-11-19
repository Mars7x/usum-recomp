This is the source shipped in gilrs-core 0.6.8, derived from
[upstream 07e286e](https://gitlab.com/gilrs-project/gilrs/-/tree/07e286e24b046cf39e5c367daa2770b805a64692/gilrs-core).
Copyright Mateusz Sieczko and other GilRs developers. Redistributed under
Apache-2.0. The original copyright and license notices are preserved in
the source files.

The local changes remove libudev and use the existing evdev and inotify
backend for enumeration and hotplug. Inotify now joins the main epoll set
directly. The public API and gilrs controller mappings are unchanged.
