Feature: A Then before app startup is vacuous and must fail loud

  Negative control for the assert runner. The Then runs before the app is
  started, so there is no rendered state to assert against. Strict replay must
  HARD PANIC (vacuity guard) rather than silently passing.

  Scenario: Asserting before startup
    Given an org file "vac.org":
      """
      * SomeContent
      :PROPERTIES:
      :ID: vac-blk
      :END:
      """
    Then block "block:vac-blk" contains "SomeContent"
