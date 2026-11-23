# Bypass asks nothing (design, 2026-11-22)

Status: asked by the owner on 2026-11-22: "With bypass on, Clio asks absolutely no permission, and
I am replaced by the judge in what is worth consulting the judge about." It changes S-72 and
touches S-68, S-74 and S-75. It builds on `2026-11-20-bypass-design.md`.

## What changes

Until now bypass answered by itself what a permission window answers, and more, but left to the
person what would change Clio's own guards, run code outside the jail, open a credential store,
change the person's blocks from a marked conversation, or add a skill from outside. Those waited
in the box. Now nothing waits for the person while bypass is on:

- **What bypass already answered** (`grant::bypass_answers`) is answered as before, with no judge.
- **What it left to the person** (`grant::bypass_judges`: a skill from outside, any `control` that
  is not the machine's alone, a block change from a marked conversation) is read by Clio's judge
  right when it is asked, as the box would show it (`pending::judged_text`). Clean: bypass answers
  it through the same path and record as before, and its yes counts as the person's own for that
  act (a command that names a credential store runs with it open, S-75). Flagged: the existing
  flag path (the conversation becomes `suspect`, bypass turns off everywhere), the act is not done
  and not set aside. No answer, or no judge: refused in fixed words (`grant::REFUSED_SILENT`) that
  say the judge did not answer and that under bypass nothing waits.
- **The judge's no is a refusal.** In a `suspect` or `unjudged` conversation, what would wait is
  refused in fixed words (`grant::REFUSED_SUSPECT`, `REFUSED_UNJUDGED`), never set aside. The
  same holds for a skill or service draft, an MCP proposal, a request about what Clio created and
  `skill_add` (`ToolHost::bypass_refuses`).
- **Still the person's**: a secret only they can type, the model's question, turning bypass on.
- **Unchanged**: reviews, rounds and subagents (S-73) never act under bypass; with the judge off
  bypass refuses to turn on and turns off; the first flag turns it off everywhere; permission
  windows (S-36).

## A skill from outside

The tool host judges every file when the model calls `skill_add` (S-68). Under bypass:

- every file clean: each skill is set aside with `judge: clean`, and the person's chat adds it when
  the turn ends through the person's own path (only the person's `clio` writes the skills folder,
  S-68). The tool host's second lock (`approve_made`) lets bypass's yes through only for such an
  item; a window's id is still refused.
- one file flagged: the whole call is refused, nothing is set aside, and the flag path runs.
- one file not judged: the whole call is refused, nothing is set aside.

The shared folder's look (S-74) is nobody's request: under bypass it sets nothing aside, opens no
box, drops what waited from an earlier look (asked again once bypass is off), and leaves one note
with how many skills are new or changed and that the person can ask Clio for them or run `clio
skills add ~/.agents/skills`.

## Informed consent

The `/bypass on` box says the new meaning: Clio asks nothing, her judge alone decides in the
person's place, and that includes Clio's own setup, programs outside the jail, credential stores
and skills from outside. A bypass turned on before this change has no `asks_nothing` mark in its
file: it reads as off in the status line, the envelope and the tool host, and only the new box's
yes turns it on again. `clio-host@17` says the same to the model, and that under bypass it never
tells the person to do a step themselves because of a permission.

## Decisions

- **What waited from before keeps its box.** An item of a judged kind set aside before bypass was
  on is not judged by the chat's check: bypass judges an act only right when it is asked
  (`grant::NOT_JUDGED_NOW`). Judging it later would read it without the moment it was asked in.
- **The fixed words fit in 200 characters**, since the refusal reaches the model through
  `judge::one_line`.
- **A block change is written in the tool host** as the person's yes writes it (`approve_block`),
  not set aside for the chat.
- **An act the judge did not answer for does not mark the conversation `unjudged`**: only text read
  from outside does; the act itself is refused.
- **Background jobs keep the stores closed** under bypass, as before.
- **The README** does not describe bypass, so it is not changed.

## Invariants and their tests

- Nothing waits; the judge's clean reading answers Clio's own guards and a marked block change
  through the person's path and bypass's record:
  `under_bypass_the_judge_answers_in_the_persons_place_and_nothing_waits`.
- Flagged: refused, nothing waits, `suspect`, bypass off; silent: refused, nothing waits, bypass
  on; a suspect or unjudged conversation's acts refused:
  `under_bypass_a_flagged_or_unjudged_act_is_refused_and_nothing_waits`,
  `a_flag_turns_bypass_off_and_a_suspect_session_never_acts_under_it`,
  `a_silent_judge_ends_bypass_for_that_conversation_only`, attack 70
  (`the_bypass_case_is_active_and_refused`).
- A `control` command runs with the machine's mark, or only after the judge:
  `under_bypass_a_control_command_runs_with_the_machines_mark_or_the_judges_clean_reading`.
- A store opens for bypass's yes only after the judge, never for a window:
  `only_a_yes_to_the_command_itself_opens_a_wall`.
- A skill from outside: `a_skill_from_outside_is_answered_only_by_bypass_after_the_judges_clean_reading`,
  `under_bypass_the_judges_reading_adds_a_skill_or_refuses_it`; the shared look:
  `under_bypass_the_shared_look_adds_nothing_and_leaves_one_note`.
- An old bypass is off and the box asks again: `a_bypass_from_before_it_asked_nothing_is_off`,
  `a_bypass_from_before_it_asked_nothing_reads_off_and_the_box_asks_again`.
- What bypass answers and judges, and why it leaves each item:
  `bypass_answers_more_than_a_window_and_clios_own_guards_only_after_the_judge`,
  `bypass_says_why_it_leaves_each_item_to_the_person`.
- The prompt: `the_host_prompt_only_changes_what_bypass_leaves_to_the_person`.

Each was checked by breaking the code it covers and seeing it fail.

## Not included

- A real model under bypass: the tests use a fake judge.
- A draft the brain reviews other than those named above is not refused under bypass in a suspect
  conversation; it waits as before.

## Version

Not bumped: the owner bumps the version.
