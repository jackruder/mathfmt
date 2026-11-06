How you report. You hold one piece of work, and its details go to exactly one seat: your parent.

**Your parent** is the seat that spawned you. `phalanix ps --all` shows it in the PARENT
column, and your brief item's ref names the item it holds (`item:<n>`) or the seat itself. A
seat the person spawned has no parent, and reports to lead. Parent and child reach each other
without an edge.

**Start.** `phalanix items --mine`. Your brief, and any later fix round, is usually yours
already: you accept automatically, so the ledger has taken it for you. `phalanix accept
<item>` only an item still shown as offered (`claim` takes only an open item no one was
offered). Read the brief whole. If it is unclear, ask your parent before you change
anything.

**Details go in a report attachment.** Write `report.md`:
- what you did;
- the commits, as `git log --oneline` shows them;
- what you ran and its counts, as cargo prints them (`test result: ok. N passed`);
- anything you doubted or left open.
Attach it: `phalanix attach <item> report.md --name report-<n>.md`, with `<n>` the item
number, since an item holds one attachment per name.

**Finishing.** Attach first: `done` is final, and nothing can be attached after it. Then:
- When your branch carries the result, `phalanix submit <item>`. The daemon pushes it soon
  after, as seats/mathfmt/<your seat>, and nothing leaves your clone before.
- When the result is attachments and messages only, `phalanix done <item> --reason
  completed`.
- When you are stuck, `phalanix done <item> --reason blocked-on --target item:<n>`, or
  `--reason escalated --target person` when only the person can unblock you.
- When it is not your work after all, attach what you have and hand the item back:
  `phalanix handoff <item> <parent>`.
Then tell your parent in a line or two: `phalanix send <parent> "<what is ready>" --item
<item>`. A message names things; it never carries a log, a diff or a table. Read mail with
`phalanix inbox --ack`.

**Your own children**, if you spawn any, report to you the same way. Read their reports,
fetch only the attachment you need (`phalanix fetch <item> <name>`), and once a child's item is
done, `phalanix close <child>@mathfmt` to free its slot.
