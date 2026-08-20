@wip @peripheral @documented-only
Feature: Flashcards / spaced repetition
  # From feature-inventory; the left sidebar exposes a "Flashcards" entry (observed).

  Scenario: Turn a block into a flashcard
    When I tag a block as a card (e.g. #card) with a cloze or Q/A child
    Then it becomes reviewable in the Flashcards view

  Scenario: Review scheduling
    When I review a card and rate my recall
    Then its next-review interval is rescheduled by the SRS algorithm
