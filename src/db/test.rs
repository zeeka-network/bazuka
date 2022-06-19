use super::*;

#[cfg(feature = "node")]
use tempdir::TempDir;

#[cfg(feature = "node")]
fn temp_disk_store() -> Result<LevelDbKvStore, KvStoreError> {
    LevelDbKvStore::new(TempDir::new("bazuka_test").unwrap().path(), 64)
}

#[test]
#[cfg(feature = "node")]
fn test_ram_and_disk_pair_prefix() -> Result<(), KvStoreError> {
    let mut ram = RamKvStore::default();
    let mut disk = temp_disk_store()?;

    assert_eq!(ram.checksum::<Hasher>()?, disk.checksum::<Hasher>()?);

    let ops = &[
        WriteOp::Put("bc".into(), Blob(vec![0, 1, 2, 3])),
        WriteOp::Put("aa".into(), Blob(vec![3, 2, 1, 0])),
        WriteOp::Put("a0a".into(), Blob(vec![])),
        WriteOp::Put("bge".into(), Blob(vec![])),
        WriteOp::Put("def".into(), Blob(vec![])),
    ];

    ram.update(ops)?;
    disk.update(ops)?;

    assert_eq!(disk.pairs("".into())?.len(), 5);
    assert_eq!(ram.pairs("".into())?.len(), 5);
    assert_eq!(disk.pairs("a".into())?.len(), 2);
    assert_eq!(ram.pairs("a".into())?.len(), 2);
    assert_eq!(disk.pairs("b".into())?.len(), 2);
    assert_eq!(ram.pairs("b".into())?.len(), 2);
    assert_eq!(disk.pairs("d".into())?.len(), 1);
    assert_eq!(ram.pairs("d".into())?.len(), 1);
    assert_eq!(disk.pairs("a0".into())?.len(), 1);
    assert_eq!(ram.pairs("a0".into())?.len(), 1);
    assert_eq!(disk.pairs("a1".into())?.len(), 0);
    assert_eq!(ram.pairs("a1".into())?.len(), 0);

    Ok(())
}

#[test]
#[cfg(feature = "node")]
fn test_ram_and_disk_db_consistency() -> Result<(), KvStoreError> {
    let mut ram = RamKvStore::default();
    let mut disk = temp_disk_store()?;

    assert_eq!(ram.checksum::<Hasher>()?, disk.checksum::<Hasher>()?);

    let ops = &[
        WriteOp::Put("bc".into(), Blob(vec![0, 1, 2, 3])),
        WriteOp::Put("aa".into(), Blob(vec![3, 2, 1, 0])),
        WriteOp::Put("def".into(), Blob(vec![])),
    ];

    ram.update(ops)?;
    disk.update(ops)?;

    assert_eq!(ram.checksum::<Hasher>()?, disk.checksum::<Hasher>()?);

    let new_ops = &[
        WriteOp::Remove("aa".into()),
        WriteOp::Put("def".into(), Blob(vec![1, 1, 1, 2])),
        WriteOp::Put("ghi".into(), Blob(vec![3, 3, 3, 3])),
    ];

    ram.update(new_ops)?;
    disk.update(new_ops)?;

    assert_eq!(ram.checksum::<Hasher>()?, disk.checksum::<Hasher>()?);

    Ok(())
}

#[test]
fn test_mirror_kv_store() -> Result<(), KvStoreError> {
    let mut ram = RamKvStore::default();

    let ops = &[
        WriteOp::Put("bc".into(), Blob(vec![0, 1, 2, 3])),
        WriteOp::Put("aa".into(), Blob(vec![3, 2, 1, 0])),
        WriteOp::Put("def".into(), Blob(vec![])),
    ];

    ram.update(ops)?;

    let prev_ram_checksum = ram.checksum::<Hasher>()?;

    let mut mirror = RamMirrorKvStore::new(&ram);

    let ops_on_mirror = &[
        WriteOp::Put("bc".into(), Blob(vec![0, 1, 2, 4])),
        WriteOp::Put("dd".into(), Blob(vec![1, 1, 1])),
        WriteOp::Put("ghi".into(), Blob(vec![2, 3])),
    ];

    mirror.update(ops_on_mirror)?;

    let mirror_checksum = mirror.checksum::<Hasher>()?;

    let mirror_ops = mirror.to_ops();

    assert_eq!(ram.checksum::<Hasher>()?, prev_ram_checksum);

    ram.update(&mirror_ops)?;

    assert_eq!(ram.checksum::<Hasher>()?, mirror_checksum);

    Ok(())
}

#[test]
fn test_rollback_of() -> Result<(), KvStoreError> {
    let mut ram = RamKvStore::default();

    let ops = &[
        WriteOp::Put("bc".into(), Blob(vec![0, 1, 2, 3])),
        WriteOp::Put("aa".into(), Blob(vec![3, 2, 1, 0])),
        WriteOp::Put("def".into(), Blob(vec![])),
    ];

    ram.update(ops)?;

    assert_eq!(ram.rollback_of(&[])?, vec![]);

    assert_eq!(
        ram.rollback_of(&[WriteOp::Remove("kk".into()),])?,
        vec![WriteOp::Remove("kk".into())]
    );

    assert_eq!(
        ram.rollback_of(&[
            WriteOp::Put("bc".into(), Blob(vec![3, 2, 1])),
            WriteOp::Put("gg".into(), Blob(vec![2, 2, 2, 2])),
            WriteOp::Put("fre".into(), Blob(vec![1, 1])),
            WriteOp::Remove("aa".into()),
        ])?,
        vec![
            WriteOp::Put("bc".into(), Blob(vec![0, 1, 2, 3])),
            WriteOp::Remove("gg".into()),
            WriteOp::Remove("fre".into()),
            WriteOp::Put("aa".into(), Blob(vec![3, 2, 1, 0])),
        ]
    );

    Ok(())
}
