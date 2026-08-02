# API note sync: a member posts a note via the API (POST /records/{id}/notes);
# the record's synced board shows it. Setup provisions the workspace, then
# activates the activity channel.
@e2e @api @sync-note
Feature: API — note sync
  A member posts a note through the API; it reaches the record's synced board —
  one test, one shared scope across every step.

  Scenario: A note posted via the API appears on the board
    Given the workspace is provisioned
    And the activity channel is activated and ready
    And the record W-${run:id} is resolved
    When a member posts a note to the board
    Then the service records a delivery
    And the board shows the note
