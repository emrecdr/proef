# API photo sync: a relative uploads a photo via the backend API (multipart;
# provide a JPEG via ${env:RUNTIME_PHOTO}); the client's photo feed shows it.
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
# app: backend
@e2e @api @sync-photo
Feature: API — photo sync
  A relative uploads a photo through the API; it appears in the client's photo
  feed — one test, one shared scope.

  Scenario: A photo uploaded via the API appears in the photo feed
    Given the client environment is provisioned
    And the client feed is activated and ready
    And the client Bakker-${run:id} is resolved
    When the relative uploads a photo
    Then the backend dispatches a push notification
    And the client photo feed shows the photo
