# API live session: a member starts a live session via the API; the session
# channel reports it as pending until it is cancelled.
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
@e2e @api @session
Feature: API — live session
  A member starts a live session through the API; the board reports it as
  pending until it is cancelled.

  Scenario: A live session appears as pending and can be cancelled
    Given the workspace is provisioned
    And the activity channel is activated and ready
    And the record W-${run:id} is resolved
    When a member starts a live session
    Then the board shows the pending session
    And the session can be cancelled via the API
