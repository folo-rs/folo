# Required by [script], which is required due to https://github.com/casey/just/issues/2702
set unstable

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set script-interpreter := ["pwsh"]

package := ""
target_package := if package == "" { " --workspace" } else { " -p " + package }

_default:
    @just --list

import 'justfiles/just_basics.just'
import 'justfiles/just_quality.just'
import 'justfiles/just_release.just'
import 'justfiles/just_setup.just'
import 'justfiles/just_testing.just'
