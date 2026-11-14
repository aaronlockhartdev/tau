Feature: Workspace layout

  @R-workspace-01
  Scenario: The root manifest lists its members by glob
    Given the workspace manifest
    When the members are listed
    Then the members come from the crates glob

  @R-workspace-02
  Scenario: Every crate is named after its directory
    Given the workspace manifest
    When the crates are read
    Then every crate is named after its directory

  @R-workspace-03
  Scenario: Local state stays out of git
    Given the git index
    When the local state paths are listed
    Then target and .tau are untracked
