# Worktime
## Summary
A simple worktime tracker.

It automatically records your work time based on mouse/keyboard activity.

## Concept

### Automatic activity monitoring
A work pause is detected as follows:

1. Keyboard/Mouse activity is monitored
2. Periods of mouse/keyboard inactivity are classified as
    - Worktime if they are shorter than `timeout_minutes` (default=10).
    - Pause if they are longer
3. Classification is done as soon as mouse/keyboard activity is detected again after idle time
4. A new worktime entry is started after every pause

#### Example:
Times when keyboard/mouse activity was recognized:
```
10:00:00
10:00:02
10:05:00
10:15:01
10:18:01
```
Will lead to the following classification (timeout_minutes=10):
```
10:00:00 ... 10:05:00 (5 minutes working)
    Pause (10min 1s)
10:15:01 ... 10:18:01 (3 minutes working)
```

### User interface

#### Reading worktime:
The data storage is kept in memory and provides filtering/formatting functions.

Currently the only implemented output is to print a summary of the current day:

Example user output:
```
 Start: 2023-01-23 08:39:43 End: 10:52:18 (2h12m35s) "Meetings, Some coding"
  Pause: 0h20m2s
 Start: 2023-01-23 11:12:21 End: 12:17:40 (1h5m19s) "Read about cool stuff"
  Pause: 0h36m27s
 Start: 2023-01-23 12:54:08 End: 13:06:24 (0h12m15s) ""
Current: Day: 3h30m10s, Week: 3h30m10s, Month: 3h30m1
```

This is periodically printed to stdout when the tool is running.

I plan to implement a better frontend as soon as I have some more time... I am also happy to accept PRs ;)

#### Editing worktime:
The tool mostly manages data capture automatically. The idea is to avoid heavy manual editing of worktime. If you want to do this, this tool might not be what you are looking for.

However there is some control over worktime entries:

- Quitting the program ("Ctrl"+"c") will finish the current work block, and store the entry to file (thus causing a pause)
- Commenting a worktime entry will be supported in the future.
- You might want to write/use external software to bulk-edit the data storage file directly (e.g. legalizing worktime).

### Data storage
A Worktime entry is the fundamental storage unit. It consists of:

- start time/date (Local time)
- end time/date (Local time)
- comments (One string only, due to limitations of CSV)

The data store maintains a list of Worktime entries (currently serialized as CSV).

If a working block is not yet finished, it will be stored as a worktime entry anyways, with the current date/time as end.
On next save, it will be overwritten:
The start time/date is used as index into the data store. Adding an entry with the same start time/date as the last one will overwrite it.

The data is stored in a csv-based database, which looks like this:
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

- `timeout_minutes`: number in minutes of allowed absence. After this time, the absence is counted as a break and a worktime entry (start/end times) is closed. After mouse/keyboard activity is registered again, a new worktime entry is automatically started.
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
I develop and test this under Linux/X11. I do not test on other platforms.

The only platform dependent crate this project depends on is [rdev](https://github.com/Narsil/rdev).

In theory, rdev supports:
- Linux/X11
- Windows
- MacOs

Unfortunately listening on mouse/keyboard events is not supported for linux/Wayland.

## State of development
The tool is usable. Core functionality is implemented. "It works for me", but far from a clean stable and widely usable product.

Notable missing functions are:
- Lock file to prevent multiple running instances to write to same data file
- User interface for adding comments to worktime entries
- Better Frontend
