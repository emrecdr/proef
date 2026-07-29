# Search matrix: proef-native coverage for Scenario Outlines, data tables, and
# the built-in expectStatus assert macro (Then-merge).
# baseURL: ${env:PROEF_BASE_URL:-http://127.0.0.1:8787}
@api @search
Feature: API — admin search matrix

  Background:
    Given the client environment is provisioned

  Scenario: Search accepts an explicit index via a data table
    When the admin searches for "Jansen, A."
      | index | clients |
    Then the response status is 200

  Scenario Outline: Search over <index> returns <status>
    When the admin searches for <term>
      | index | <index> |
    Then the response status is <status>

    Examples:
      | index    | term    | status |
      | clients  | Bakker  | 200    |
      | users    | de Vries| 200    |
      | missing  | nobody  | 404    |
