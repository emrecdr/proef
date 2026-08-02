# API event sync: a member adds a scheduled item via the API
# (POST /records/{id}/events); the record's board shows it.
@e2e @api @sync-event
Feature: API — event sync
  A member schedules an item through the API; it appears on the record's synced
  board — one test, one shared scope.

  Scenario: A scheduled item created via the API appears on the board
    Given the workspace is provisioned
    And the activity channel is activated and ready
    And the record W-${run:id} is resolved
    When a member adds a scheduled item
    Then the service records a delivery
    And the board shows the scheduled item
