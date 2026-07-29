# Breadth coverage (M5): docstring bodies, form + multipart uploads, fakes.
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
@api @breadth
Feature: API — breadth (bodies, forms, uploads, fakes)

  Scenario: A custom message body is sent from a docstring
    When the relative sends a custom message
      """
      {"body": "Fijne verjaardag!", "priority": "high"}
      """
    Then the response status is 201

  Scenario: A profile form is submitted
    When the profile form is submitted for Bakker
    Then the response looks healthy

  Scenario: A photo file is uploaded as multipart
    When the photo file is uploaded
    Then the response status is 201

  Scenario: A synthetic client name searches cleanly
    When the admin searches for ${fake:lastName}
    Then the response status is 200
