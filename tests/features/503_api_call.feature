# API incoming call: a relative places a video call via the backend; the call
# feed reports it as incoming until it is denied.
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
# app: backend
@e2e @api @call
Feature: API — incoming video call
  A relative places a call through the API; the call feed reports it as incoming
  until it is denied.

  Scenario: A call invite appears in the call feed and can be denied
    Given the client environment is provisioned
    And the client feed is activated and ready
    And the client Bakker-${run:id} is resolved
    When the relative places a video call
    Then the call feed shows the incoming call
    And the call can be denied via the API
