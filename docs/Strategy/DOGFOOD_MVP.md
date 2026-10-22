# Dogfood MVP

This file contains a description of the tasks I need to work properly in order to be able to dogfood
Holon for myself as daily work planner & tracker.

## Journal Overview
The Journal should look like in LogSeq where I see all days (lazily loaded) below each other, newest on top.
Each day has the date as heading (on click opens that day in the main panel), then the content and at the end a separator line.

## New day
I have a template for starting my day:
`/Users/martin/Workspaces/pkm/silverbullet-pkm/Templates/Plan my day.md`
This needs to be converted to a `Plan my day` template that I can easily inject into my Journal pages.
Marking tasks as in progress / done needs to work.

## New month
Same for starting a new month using `/Users/martin/Workspaces/pkm/silverbullet-pkm/Templates/NewMonth.md`.

## Project pages
I need to be able to easily create a new project from the Journal.
I would like to be able to write `[[Projects/My new project]]` and have that become a clickable link
which creates the block as a page.
<!--
I think the fact that I'm referencing something that does not exist yet but is in a hierarchy should be sufficient to make it clear that I'm talking about a page.
Do you agree?
Actually one could not even create a non-page block with `[[...]]` because it would not be clear where it would live, right?
-->
