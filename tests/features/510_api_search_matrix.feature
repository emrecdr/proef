# Search matrix: proef-native coverage for Scenario Outlines, data tables, and
# the built-in expectStatus assert macro (Then-merge).
@api @search
Feature: API — admin search matrix

  Background:
    Given the workspace is provisioned

  Scenario: Search accepts an explicit index via a data table
    When the operator searches for "Acme, Inc."
      | index | records |
    Then the response status is 200

  Scenario Outline: Search over <index> returns <status>
    When the operator searches for <term>
      | index | <index> |
    Then the response status is <status>

    Examples:
      | index    | term    | status |
      | records  | Acme    | 200    |
      | people   | Jordan  | 200    |
      | missing  | nobody  | 404    |
