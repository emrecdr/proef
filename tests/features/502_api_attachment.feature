# API attachment sync: a member uploads an attachment via the API (JSON body — real
# multipart upload lives in the 520 breadth suite); the record's board shows it.
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
@e2e @api @sync-attachment
Feature: API — attachment sync
  A member uploads an attachment through the API; it appears on the record's
  synced board — one test, one shared scope.

  Scenario: An attachment uploaded via the API appears on the board
    Given the workspace is provisioned
    And the activity channel is activated and ready
    And the record W-${run:id} is resolved
    When a member uploads an attachment
    Then the service records a delivery
    And the board shows the attachment
