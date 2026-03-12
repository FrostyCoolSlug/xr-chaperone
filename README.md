# xr-chaperone

Blarp. Have rust, make sure monado (or other overlay compatible XR compositor) is running, then run:

```bash
cargo run --release
```

Initial configuration will happen on your monitor, walk around your space with a controller, and pull the trigger to
set corner points.

Double pull the trigger on your last point to finish (or match up with the first point).

One done, perform any tuning you want on the screen, then hit save.

Once done, the chaperone should be present in your VR space.

You can do bonus config from the settings button to tweak stuff.

Warning: May segfault occasionally, try not to do anything too crazy. If you can reproduce, open an issue here :)

Tested with the Valve index, should work with others, but I have no way to test. If it doesn't work with your headset,
open an issue, and I'll see if I can work it out.

I'll build an AppImage soonish (once I work out how :D), and add an icon at some point, and a desktop file so it can
be integrated into envision for autostart, but for now, just run from source.

That is all.