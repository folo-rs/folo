set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]

_default:
    @just --list

import 'just_basics.just'
import 'just_quality.just'
import 'just_setup.just'
import 'just_testing.just'
