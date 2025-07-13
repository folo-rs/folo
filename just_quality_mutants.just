[group('quality')]
[script]
mutants:
    function Escape-Wildcards ($s) {
        if (!$IsLinux) {
            return $s
        }

        # On Linux, PowerShell has built-in globbing that expands wildcards. Unfortunately,
        # cargo mutants requires literal input values, globbing just breaks it. We convince
        # PowerShell to turn off globbing by single-quoting the arguments we fear may be
        # interpreted as wildcard glob expressions.
        "'" + $s + "'"
    }

    # Note: while cargo-mutants does theoretically support specifying these via config file, it
    # does not support merging that static set of entries with the dynamic set of entries we
    # determine based on the platform. So we have to specify them all here.
    # https://github.com/sourcefrog/cargo-mutants/issues/527
    $args = @(
        # Parts of this package require Criterion to work and other parts are currently not tested
        # as there is no public way to simulate a system topology for `many_cpus`.
        "-e"
        "many_cpus_benchmarking",

        # Macros are tested via the impl package, mutations in the middle layer might not be detected.
        "-e"
        "linked_macros"

        # We do not test facades, as they are just trivial code that forwards calls to real impls.
        "-e"
        (Escape-Wildcards "**/*facade.rs")
        "-e"
        "facade"

        # We have limited coverage of platform bindings because it can be difficult to set up the
        # right scenarios for each, given they are platform-dependent. Instead, we test higher
        # level code using a mock platform.
        "-e"
        "bindings"

        # This is just a different type of bindings, skipped for same reason as `bindings` above.
        "-e"
        (Escape-Wildcards "packages/many_cpus/src/pal/linux/filesystem/**")

        # This package is literally full of synchronization primitives, so 95% of mutations will
        # just cause tests to time out and never complete - pointless to try mutate this stuff.
        "-e"
        "events"
        
        # All this is code only used in tests/benchmarks - we do not test this code itself.
        "-e"
        (Escape-Wildcards "packages/testing/**")
        "-e"
        (Escape-Wildcards "packages/benchmarks/**")
    )

    if ($IsLinux) {
        $args += "-e"
        $args += (Escape-Wildcards "**/*windows.rs")

        $args += "-e"
        $args += "windows"
    } else {
        $args += "-e"
        $args += (Escape-Wildcards "**/*linux.rs")

        $args += "-e"
        $args += "linux"
    }

    # Heuristically, 4 processors per job seems to work out well to achieve high efficiency.
    $args += "--jobs"
    $args += [Math]::Max([Environment]::ProcessorCount / 4, 1)

    # We must use Invoke-Expression to preserve the quotes around the wildcarded arguments on Linux.
    $expanded_args = [String]::join(" ", $args)

    # If the user has overridden `CARGO_TARGET_DIR`, we need to un-override it here because it is
    # critical that different mutations be emitted into different target directories, otherwise
    # the mutation test jobs will conflict with each other, running each others' binaries.
    if ($env:CARGO_TARGET_DIR) {
        $env:CARGO_TARGET_DIR = $null

        # If the user set a custom `CARGO_TARGET_DIR` they probably did it because there is a more
        # performant storage path available than the default, in which case they will also want to
        # specify `MUTANTS_TEMP` to direct mutation testing to the more performant storage.
        if (!$env:MUTANTS_TEMP) {
            Write-Warning "Consider setting a value for the MUTANTS_TEMP environment variable, as you have set CARGO_TARGET_DIR to a custom value. CARGO_TARGET_DIR is ignored during mutation testing."
        }
    }

    # If the user has specified a custom `MUTANTS_TEMP` directory, we need to set it here.
    # This is useful for performance, as it allows mutation testing to use a faster storage path
    # than the default (e.g. because real-time antivirus is not scanning it).
    if ($env:MUTANTS_TEMP) {
        # TMP is the variable used by cargo-mutants to determine where to put the files.
        $env:TMP = $env:MUTANTS_TEMP
    }

    # Set environment variable to disable watchdog during mutation testing.
    # This allows mutations that cause infinite loops to hang as expected.
    $env:MUTATION_TESTING = "1"

    # --baseline=skip - We assume that regular tests have already been executed
    #     and are not failing, for mutation speedup.
    # --no-shuffle - We do not care about shuffling, that is something rarely useful.
    # --caught --unviable - We want to see a list of all the mutations tried, as it helps us
    #     understand what is covered by mutation testing and how successfully.
    # --timeout - as we skip the baseline tests, we need to manually define a timeout.
    #     This needs to be long enough to run a full package test suite (for all jobs in parallel)
    #     but short enough to call a timeout and timeout and avoid things going on forever.
    #     This should only be reached if a mutation causes a serious issue (and may need to be
    #     skipped).
    Invoke-Expression "cargo mutants {{ target_package }} --baseline=skip --timeout=60 --no-shuffle --caught --unviable $expanded_args"
    exit $LASTEXITCODE
