set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
shebang := if os() == "windows" { "pwsh.exe" } else { "/usr/bin/env pwsh" }

package := ""
target_package := if package == "" { " --workspace" } else { " -p " + package }

_default:
    @just --list

import 'just_basics.just'
import 'just_quality.just'
import 'just_setup.just'
import 'just_testing.just'
