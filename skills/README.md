# uncurses skill

The [uncurses skill](uncurses/SKILL.md) helps an assistant write applications
that depend on uncurses. It explains the ownership model, safe teardown,
common API mistakes, and where to find examples.

The skill complements the [website](https://uncurses.org) and API reference.
It targets the repository source. Check your dependency revision before using
its samples with a published release.

## Install the skill

The repository distributes the skill in `skills/uncurses/`, with `SKILL.md`
as its entry point. Use your assistant's documented installation procedure
for skills. Choose a project installation for one application or a personal
installation for use across projects, if your assistant supports these options.

If your assistant installs skills from a directory, select `skills/uncurses/`.
If it accepts a skill file, select `skills/uncurses/SKILL.md`.
For a manual installation, copy the whole `uncurses` directory into the skill
location documented by your assistant. Keep `SKILL.md` and any supporting
files together.

Use your assistant's skill list or discovery procedure to confirm that it
finds `uncurses`.

## Update the skill

After you update the uncurses checkout, refresh the installed skill through
your assistant's update procedure. For a manually copied installation, replace
the installed directory with the updated `skills/uncurses/` directory.
Check the application's dependency revision against the skill's source revision
before you use updated samples.

## Work on the repository

For uncurses development, start with [AGENTS.md](../AGENTS.md) and
[CONTRIBUTING.md](../CONTRIBUTING.md). They contain repository rules, check
commands, and routes to implementation contracts.
