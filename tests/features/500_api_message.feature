# API message sync: a relative sends a message via the backend API; the client's
# synced feed shows it. Setup provisions the environment, then activates the feed.
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
# app: backend
@e2e @api @sync-message
Feature: API — message sync
  A relative sends a message through the API; it reaches the client's synced feed —
  one test, one shared scope across every step.

  Scenario: A message sent via the API appears in the client feed
    Given the client environment is provisioned
    And the client feed is activated and ready
    And the client Bakker-${run:id} is resolved
    When the relative sends a message to the client
    Then the backend dispatches a push notification
    And the client feed shows the message from the relative
