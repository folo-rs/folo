use std::cell::RefCell;

use super::*;

fn regular() -> InputMetadata {
    InputMetadata {
        regular: true,
        symlink: false,
        directory: false,
        #[cfg(unix)]
        executable: false,
    }
}

fn encode(
    metadata: io::Result<InputMetadata>,
    contents: io::Result<Vec<u8>>,
    replacement: Option<Vec<u8>>,
) -> Result<Vec<u8>, AppError> {
    encode_with_modes(
        metadata,
        contents,
        replacement,
        &WorkTreeModes::default(),
        false,
        false,
    )
}

fn encode_with_modes(
    metadata: io::Result<InputMetadata>,
    contents: io::Result<Vec<u8>>,
    replacement: Option<Vec<u8>>,
    modes: &WorkTreeModes,
    allow_symlink: bool,
    allow_directory: bool,
) -> Result<Vec<u8>, AppError> {
    let root = Path::new("root");
    let path = PathBuf::from("input");
    let mut metadata = Some(metadata);
    let contents = RefCell::new(Some(contents));
    let symlink_inputs = if allow_symlink {
        BTreeSet::from([path.clone()])
    } else {
        BTreeSet::new()
    };
    let directory_inputs = if allow_directory {
        BTreeSet::from([path.clone()])
    } else {
        BTreeSet::new()
    };
    fingerprint_bytes(
        root,
        &BTreeSet::from([path.clone()]),
        &replacement
            .map(|bytes| BTreeMap::from([(path.clone(), bytes)]))
            .unwrap_or_default(),
        &PathIdentity::new(root, &|_| panic!("exact names require no probe")),
        modes,
        &symlink_inputs,
        &directory_inputs,
        |path| {
            assert_eq!(path, root.join("input"));
            metadata.take().unwrap()
        },
        |path| {
            assert_eq!(path, root.join("input"));
            contents.borrow_mut().take().unwrap()
        },
        |path| {
            assert_eq!(path, root.join("input"));
            contents.borrow_mut().take().unwrap()
        },
    )
}

#[test]
fn metadata_errors_are_not_missing_files_even_with_a_replacement() {
    for kind in [
        ErrorKind::PermissionDenied,
        ErrorKind::NotADirectory,
        ErrorKind::Other,
    ] {
        for replacement in [None, Some(b"replacement".to_vec())] {
            let error = encode(Err(kind.into()), Ok(Vec::new()), replacement).unwrap_err();
            assert!(error.find_source::<ReadFileError>().is_some());
            assert_eq!(error.find_source::<io::Error>().unwrap().kind(), kind);
        }
    }
}

#[test]
fn byte_read_errors_propagate_including_disappearance_after_metadata() {
    for kind in [ErrorKind::NotFound, ErrorKind::PermissionDenied] {
        let error = encode(Ok(regular()), Err(kind.into()), None).unwrap_err();
        assert!(error.find_source::<ReadFileError>().is_some());
        assert_eq!(error.find_source::<io::Error>().unwrap().kind(), kind);
    }
}

#[test]
fn replacements_do_not_admit_directories() {
    let metadata = InputMetadata {
        regular: false,
        symlink: false,
        directory: true,
        #[cfg(unix)]
        executable: false,
    };
    let error = encode(Ok(metadata), Ok(Vec::new()), Some(Vec::new())).unwrap_err();
    assert!(error.find_source::<UnsupportedInput>().is_some());
}

#[test]
fn native_and_placeholder_symlinks_encode_the_same_target_bytes() {
    let native = InputMetadata {
        regular: false,
        symlink: true,
        directory: false,
        #[cfg(unix)]
        executable: false,
    };
    let mut modes = WorkTreeModes::default();
    modes.set("input", "120000");
    let placeholder = regular();

    let native = encode_with_modes(
        Ok(native),
        Ok(b"target".to_vec()),
        None,
        &WorkTreeModes::default(),
        true,
        false,
    )
    .unwrap();
    let placeholder = encode_with_modes(
        Ok(placeholder),
        Ok(b"target".to_vec()),
        None,
        &modes,
        true,
        false,
    )
    .unwrap();
    assert_eq!(native, placeholder);
    assert_ne!(
        native,
        encode(Ok(regular()), Ok(b"target".to_vec()), None).unwrap()
    );
}

#[test]
fn unvalidated_symlinks_remain_unsupported_inputs() {
    let native = InputMetadata {
        regular: false,
        symlink: true,
        directory: false,
        #[cfg(unix)]
        executable: false,
    };
    let error = encode(Ok(native), Ok(b"target".to_vec()), None).unwrap_err();
    assert!(error.find_source::<UnsupportedInput>().is_some());
}

#[test]
fn validated_directories_encode_presence_without_contents() {
    let directory = InputMetadata {
        regular: false,
        symlink: false,
        directory: true,
        #[cfg(unix)]
        executable: false,
    };
    let encoded = encode_with_modes(
        Ok(directory),
        Err(ErrorKind::Other.into()),
        None,
        &WorkTreeModes::default(),
        false,
        true,
    )
    .unwrap();
    // The persisted directory marker has no content field, unlike even an empty regular file.
    let mut expected = vec![5, 0, 0, 0, 0, 0, 0, 0, b'i', b'n', b'p', b'u', b't', 3];
    #[cfg(unix)]
    expected.push(0);
    expected.push(0);
    assert_eq!(encoded, expected);
}

#[test]
fn resolver_control_inputs_are_never_package_link_exceptions() {
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        ".cargo/config",
        ".cargo/config.toml",
        "nested/Cargo.toml",
    ] {
        assert!(resolver_control_input(Path::new(path)));
    }
    for path in [
        "src/lib.rs",
        "build.rs",
        "tests/fixture",
        "config",
        "config.toml",
        "other/config",
        "other/config.toml",
        ".cargo/other",
        "nested/.cargo/other",
    ] {
        assert!(!resolver_control_input(Path::new(path)));
    }
}

#[test]
fn fingerprint_encoding_retains_names_presence_lengths_and_contents() {
    let missing = encode(
        Err(ErrorKind::NotFound.into()),
        Err(ErrorKind::Other.into()),
        None,
    )
    .unwrap();
    // This is the persisted fingerprint format, not a digest tied to the host's Git process.
    let prefix = vec![5, 0, 0, 0, 0, 0, 0, 0, b'i', b'n', b'p', b'u', b't'];
    let mut expected_missing = prefix.clone();
    expected_missing.push(0);
    #[cfg(unix)]
    expected_missing.push(0);
    expected_missing.push(0);
    assert_eq!(missing, expected_missing);

    let empty = encode(Ok(regular()), Ok(Vec::new()), None).unwrap();
    let mut expected_empty = prefix.clone();
    expected_empty.push(1);
    #[cfg(unix)]
    expected_empty.push(0);
    expected_empty.extend([1, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(empty, expected_empty);
    assert_ne!(missing, empty);

    let contents = encode(Ok(regular()), Ok(b"abc".to_vec()), None).unwrap();
    let mut expected_contents = prefix;
    expected_contents.push(1);
    #[cfg(unix)]
    expected_contents.push(0);
    expected_contents.extend([1, 3, 0, 0, 0, 0, 0, 0, 0, b'a', b'b', b'c']);
    assert_eq!(contents, expected_contents);
    for metadata in [Ok(regular()), Err(ErrorKind::NotFound.into())] {
        assert_eq!(
            encode(
                metadata,
                Err(ErrorKind::Other.into()),
                Some(b"abc".to_vec())
            )
            .unwrap(),
            contents
        );
    }
}

#[test]
fn executable_mode_observes_each_execute_bit_and_ignores_other_bits() {
    for mode in [0, 0o644, 0o600, 0o100644] {
        assert!(!executable_mode(mode));
    }
    for mode in [0o100, 0o010, 0o001, 0o111, 0o755, 0o100755] {
        assert!(executable_mode(mode));
    }
}

#[test]
fn only_regular_executable_inputs_contribute_the_execute_bit() {
    assert!(fingerprint_executable(true, false));
    assert!(!fingerprint_executable(false, false));
    assert!(!fingerprint_executable(true, true));
    assert!(!fingerprint_executable(false, true));
}

#[test]
#[cfg(unix)]
fn executable_state_participates_even_when_contents_are_replaced() {
    let plain = encode(Ok(regular()), Ok(b"same".to_vec()), None).unwrap();
    let metadata = InputMetadata {
        executable: true,
        ..regular()
    };
    let executable = encode(Ok(metadata), Ok(b"same".to_vec()), None).unwrap();
    assert_ne!(plain, executable);
    let metadata = InputMetadata {
        executable: true,
        ..regular()
    };
    assert_eq!(
        encode(
            Ok(metadata),
            Err(ErrorKind::Other.into()),
            Some(b"same".to_vec())
        )
        .unwrap(),
        executable
    );
}
