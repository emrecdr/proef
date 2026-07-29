# API calendar sync: a relative schedules an appointment via the backend API
# (POST /clients/{id}/agenda/events); the client's calendar feed shows it.
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
# app: backend
@e2e @api @sync-calendar
Feature: API — calendar sync
  A relative schedules an appointment from the backend; it appears on the activated
  client's calendar feed — one test, one shared scope.

  Scenario: An appointment created via the API appears in the calendar feed
    Given the client environment is provisioned
    And the client feed is activated and ready
    And the client Bakker-${run:id} is resolved
    When the relative schedules an appointment
    Then the backend dispatches a push notification
    And the client calendar feed shows the appointment
