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

  @R-workspace-06
  Scenario: A project tau opens gains an ignore file that git does not read
    Given a git project directory
    Then the tau directory is ignored by git
    And the project directory is removed

  @R-workspace-07
  Scenario: An ignore file that is already there is left as it is
    Given a git project directory with an edited ignore file
    When the project is opened again
    Then the ignore file is unchanged
    And the project directory is removed

  # Untagged: R-workspace-08 speaks of session start output, and nothing prints it
  # while the `headless session` gate is M0, so no tag would be honest here.
  Scenario: The write of the ignore file is reported
    Given a git project directory
    Then the write is disclosed
    When the project is opened again
    Then the reopen discloses nothing
    And the project directory is removed
