# Breadth coverage (M5): docstring bodies, form + multipart uploads, fakes.
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
@api @breadth
Feature: API — breadth (bodies, forms, uploads, fakes)

  Scenario: A custom note body is sent from a docstring
    When a member posts a custom note
      """
      {"body": "A custom note body", "priority": "high"}
      """
    Then the response status is 201

  Scenario: A profile form is submitted
    When the profile form is submitted for Acme
    Then the response looks healthy

  Scenario: An attachment file is uploaded as multipart
    When the attachment file is uploaded
    Then the response status is 201

  Scenario: A synthetic name searches cleanly
    When the operator searches for ${fake:lastName}
    Then the response status is 200
