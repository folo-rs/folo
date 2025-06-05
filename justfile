# Required by [script], which is required due to https://github.com/casey/just/issues/2702
set unstable

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set script-interpreter := ["pwsh"]

package := ""
target_package := if package == "" { " --workspace" } else { " -p " + package }

_default:
    @just --list

import 'just_basics.just'
import 'just_quality.just'
import 'just_release.just'
import 'just_setup.just'
import 'just_testing.just'
