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

    $args = @(
        "-e"
        # Parts of this crate require Criterion to work and other parts are currently not tested
        # as there is no public way to simulate a system topology for `many_cpus`.
        "many_cpus_benchmarking",

        "-e"
        # Macros are tested via the impl crate, mutations in the middle layer might not be detected.
        "linked_macros"

        # We do not test facades, as they are just trivial code that forwards calls to real impls.
        "-e"
        (Escape-Wildcards "**/*facade.rs")
        "-e"
        "facade"

        "-e"
        # We have limited coverage of platform bindings because it can be difficult to set up the
        # right scenarios for each, given they are platform-dependent. Instead, we test higher
        # level code using a mock platform.
        "bindings"

        "-e"
        # This is just a different type of bindings, skipped for same reason as `bindings` above.
        (Escape-Wildcards "packages/many_cpus/src/pal/linux/filesystem/**")
        
        "-e"
        # All this is code only used in tests - we do not test this code itself.
        (Escape-Wildcards "packages/testing/**")
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

    # We deliberately do not use nextest here because it cannot run doctests.

    # Multi-job mutation on Linux does not appear to work well - it seems to cause some interference
    # between the jobs. Perhaps due to our CARGO_TARGET_DIR being shared between jobs? One job is
    # executing tests from the wrong job because they are overwriting each other? Simple fix is
    # to just use 1 job on Linux, which is good enough for now.
    if ($IsLinux) {
        $args += "--jobs"
        $args += "1"
    } else {
        $args += "--jobs"
        $args += "4"
    }

    # We must use Invoke-Expression to preserve the quotes around the wildcarded arguments on Linux.
    $expanded_args = [String]::join(" ", $args)
    Invoke-Expression "cargo mutants {{ target_package }} --profile=mutants --no-times --caught --unviable $expanded_args"
