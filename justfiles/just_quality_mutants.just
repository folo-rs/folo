[group('quality')]
[script]
mutants SHARD="" CAREFUL='false':
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    $careful = '{{ CAREFUL }}' -eq 'true'

    function Escape-Wildcards ($s) {
        if ($IsWindows) {
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
        (Escape-Wildcards "packages/many_cpus_impl/src/pal/linux/filesystem/**")

        # These packages are literally full of synchronization primitives, so 95% of mutations
        # will just cause tests to time out and never complete - pointless to try mutate this stuff.
        "-e"
        "events_once",
        "-e"
        "awaiter_set",
        "-e"
        "events"
        
        # All this is code only used in tests/benchmarks - we do not test this code itself.
        "-e"
        (Escape-Wildcards "packages/testing/**")
        "-e"
        (Escape-Wildcards "packages/benchmarks/**")

        # The mock benchmark engine is a test-support binary (a `[[bin]]` so the
        # integration tests can spawn it). It is scaffolding, not shipped code, so mutating it
        # yields no production coverage signal - it only spawns extra subprocess-heavy test runs
        # that time out on slower runners. Its own behavior is covered by the integration tests.
        "-e"
        (Escape-Wildcards "packages/cargo-bench-history/src/bin/**")

        # The cargo-bench-history stress harness is an on-demand (`publish = false`) binary that
        # seeds a giant synthetic history and times each analyze mode by hand - it is never run in
        # `just test` or CI and has no unit tests of its own (its purpose is manual scaling
        # measurement, not shipped behavior). It only enters the mutation run when a change to
        # `cargo-bench-history` makes `cargo-delta` pull in this dependent package, so exclude it to
        # match the test-support treatment of the other harness/benchmark packages above.
        "-e"
        (Escape-Wildcards "packages/cargo-bench-history-stress/**")

        # Some of our systems are single-processor, yet the code may only be meaningfully testable
        # on multi-processor systems. As a "good enough" approximation, we skip mutation testing
        # of code that is only testable in a multi-processor system.
        "-e",
        (Escape-Wildcards "packages/par_bench/src/resource_usage_ext.rs")
    )

    if (!$IsWindows) {
        $args += "-e"
        $args += (Escape-Wildcards "**/*windows.rs")

        $args += "-e"
        $args += "windows"
    }
    
    if (!$IsLinux) {
        $args += "-e"
        $args += (Escape-Wildcards "**/*linux.rs")

        $args += "-e"
        $args += "linux"
    }

    $args += "--jobs"
    if ($careful) {
        # In careful mode, only one mutation is tested at a time. Combined with
        # `RUST_TEST_THREADS=1` set below, this ensures that exactly one test runs at any
        # given moment across the entire mutation testing run.
        $args += "1"
    } else {
        # Experimentally determined heuristics.
        $args += [int]([Math]::Floor([Environment]::ProcessorCount / 6)) + 1
    }

    # Optional sharding lets CI split mutation testing across multiple parallel runners.
    # We accept the same 1-based "N/M" format as `miri-harder` for consistency with the
    # existing workspace convention (so `1/8 .. 8/8` for 8 shards), and convert to
    # cargo-mutants' native 0-based form (`0/N .. (N-1)/N`) before invoking the tool.
    # Without a SHARD argument, cargo-mutants runs every mutant in a single job (the
    # original behavior).
    $shard = "{{ SHARD }}"
    if ($shard -ne "") {
        $parts = $shard -split "/"

        if ($parts.Length -ne 2) {
            Write-Error "Invalid SHARD value '$shard'. Expected format 'N/M'."
            exit 1
        }

        $shardIndex = 0
        $shardCount = 0
        if (-not [int]::TryParse($parts[0], [ref]$shardIndex) -or -not [int]::TryParse($parts[1], [ref]$shardCount)) {
            Write-Error "Invalid SHARD value '$shard'. N and M must be integers in 'N/M'."
            exit 1
        }

        if ($shardCount -le 0) {
            Write-Error "Invalid SHARD value '$shard'. M must be a positive integer in 'N/M'."
            exit 1
        }

        if ($shardIndex -lt 1 -or $shardIndex -gt $shardCount) {
            Write-Error "Invalid SHARD value '$shard'. N must satisfy 1 <= N <= M in 'N/M'."
            exit 1
        }

        $args += "--shard"
        $args += ("{0}/{1}" -f ($shardIndex - 1), $shardCount)
    }

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

    if ($careful) {
        # Force libtest to run a single test at a time within each `cargo test` invocation.
        # Combined with `--jobs 1` above, this ensures that exactly one test runs at any
        # given moment, which helps diagnose tests that are sensitive to concurrent
        # execution or otherwise interact in unexpected ways.
        $env:RUST_TEST_THREADS = "1"
    }

    # --baseline=skip - We assume that regular tests have already been executed
    #     and are not failing, for mutation speedup.
    # --no-shuffle - We do not care about shuffling, that is something rarely useful.
    # --caught --unviable - We want to see a list of all the mutations tried, as it helps us
    #     understand what is covered by mutation testing and how successfully.
    # --timeout - as we skip the baseline tests, we need to manually define a timeout.
    #     This needs to be long enough to run a full package test suite (for all jobs in parallel)
    #     but short enough to call a timeout and timeout and avoid things going on forever.
    #     This should only be reached if a mutation causes a serious issue (and may need to be
    #     skipped or the test improved).
    Invoke-Expression "cargo mutants {{ target_package }} --baseline=skip --timeout=60 --no-shuffle --caught --unviable $expanded_args"
    exit $LASTEXITCODE

# Run mutation tests with no parallelism: one mutation at a time, one test at a time within each mutation. Slow but useful for investigating flaky or concurrency-sensitive tests.
[group('quality')]
mutants-careful: (mutants "" "true")
