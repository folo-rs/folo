use super::*;

fn regular() -> InputMetadata {
    InputMetadata {
        regular: true,
        symlink: false,
        #[cfg(unix)]
        executable: false,
    }
}

fn encode(
    metadata: io::Result<InputMetadata>,
    contents: io::Result<Vec<u8>>,
    replacement: Option<Vec<u8>>,
) -> Result<Vec<u8>, AppError> {
    let root = Path::new("root");
    let path = PathBuf::from("input");
    let mut metadata = Some(metadata);
    let mut contents = Some(contents);
    fingerprint_bytes(
        root,
        &BTreeSet::from([path.clone()]),
        &replacement
            .map(|bytes| BTreeMap::from([(path, bytes)]))
            .unwrap_or_default(),
        &PathIdentity::new(root, &|_| panic!("exact names require no probe")),
        |path| {
            assert_eq!(path, root.join("input"));
            metadata.take().unwrap()
        },
        |path| {
            assert_eq!(path, root.join("input"));
            contents.take().unwrap()
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
fn replacements_do_not_admit_directories_or_symlinks() {
    for (regular, symlink) in [(false, false), (false, true), (true, true)] {
        let metadata = InputMetadata {
            regular,
            symlink,
            #[cfg(unix)]
            executable: false,
        };
        let error = encode(Ok(metadata), Ok(Vec::new()), Some(Vec::new())).unwrap_err();
        assert!(error.find_source::<UnsupportedInput>().is_some());
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
    let mut prefix = vec![5, 0, 0, 0, 0, 0, 0, 0, b'i', b'n', b'p', b'u', b't'];
    #[cfg(unix)]
    prefix.push(0);
    let mut expected_missing = prefix.clone();
    expected_missing.push(0);
    assert_eq!(missing, expected_missing);

    let empty = encode(Ok(regular()), Ok(Vec::new()), None).unwrap();
    let mut expected_empty = prefix.clone();
    expected_empty.extend([1, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(empty, expected_empty);
    assert_ne!(missing, empty);

    let contents = encode(Ok(regular()), Ok(b"abc".to_vec()), None).unwrap();
    prefix.extend([1, 3, 0, 0, 0, 0, 0, 0, 0, b'a', b'b', b'c']);
    assert_eq!(contents, prefix);
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
