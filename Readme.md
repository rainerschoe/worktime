# Worktime
## Summary
A simple worktime tracker.

It automatically  records your work time based on mouse/keyboard activity.

Example user output:
```
 Start: 2023-01-23 08:39:43 End: 10:52:18 (2h12m35s) "Meetings, Some coding"
  Pause: 0h20m2s
 Start: 2023-01-23 11:12:21 End: 12:17:40 (1h5m19s) "Read about cool stuff"
  Pause: 0h36m27s
 Start: 2023-01-23 12:54:08 End: 13:06:24 (0h12m15s) ""
Current: Day: 3h30m10s, Week: 3h30m10s, Month: 3h30m1
```

The data is stored in a csv-based database:
```
2023-01-23T08:39:43.411602735+01:00,2023-01-23T10:52:18.508372583+01:00,"Meetings, Some coding"
2023-01-23T11:12:21.348136902+01:00,2023-01-23T12:17:40.865009498+01:00,"Read about cool stuff"
2023-01-23T12:54:08.544242070+01:00,2023-01-23T13:06:24.430300568+01:00,
```

## Scope
#### What this tool tries to offer:
- Give an overview of time worked
- Allows you to find out if you work too much / not enough
- Gives feeling of how much over-time you accumulated
- Simple CLI frontend

#### What this tool does not offer:
- Precise logging of time worked (worktime is only estimated based on keyboard activity)
- Book time worked for multiple projects (although comments will be supported soon, they are informational only)
- Editing / Adjusting time entries
- Graphical user interface

## Configuration
The configuration file is placed in your operating system's default path. Unter linux this typically is `~/.config/worktime/default-config.toml`.

The following options can be configured:

- `timeout_minues`: number in minutes of allowed absence. After this time, the absence is counted as a break and a worktime entry (start/end times) is closed. After mouse/keyboard activity is registered again, a new worktime entry is automatically started.
- `data_file`: path to a `.csv` file which is used as storage of worktime entries.
- `auto_save_interval_seconds`: time period in which worktime entries are automatically stored to the data file. Note on `Ctrl`+`C` all entries are also stored.

## Building
### Dependencies
On linux/X11 you will need some xorg dependencies:
```
libXi-devel libX11-devel libXtst-devel
```

### Build
just execute `cargo build`


## Supported Platforms
I develop and test this under Lunix/X11. I do not test on other platforms.

The only platform dependent crate this project depends on is [rdev](https://github.com/Narsil/rdev).

In theory, rdev supports:
- Linux/X11
- Windows
- MacOs

Unfortunately listening on mouse/keyboard events is not supported for linux/Wayland.

## State of development
The tool is usable. Main functionality is implemented.

Notable missing functions are:
- Lock file to prevent multiple running instances to write to same data file
- User interface for adding comments to worktime entries