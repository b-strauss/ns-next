//! `cargo xtask references`: downloads the reference papers
//! `docs/references/PROVENANCE.md` lists but the repository does not
//! carry. They are third-party works, several under their publishers'
//! copyrights, so the repository only names them; each lands in
//! `docs/references/`, which `.gitignore` keeps out of the tree, under
//! the file name the design documents cite it by. A paper already there
//! whose checksum matches stays; one whose download answers other bytes
//! is refused, since the documents' section citations name the version
//! the checksum pins (`PROVENANCE.md`, "Refreshing a paper").

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// A paper: the file name it is cited by, where its author hosts it, and
/// the SHA-256 of the version the documents cite, in lowercase hex.
pub struct Paper {
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

/// The papers the task downloads, in `PROVENANCE.md`'s order.
pub const PAPERS: &[Paper] = &[
    Paper {
        file: "newspeak-modules.pdf",
        url: "https://bracha.org/newspeak-modules.pdf",
        sha256: "c46a6b9d852195338bb0fffc4ed6960d4dc6dd738fd89002bb85925a9dd7aeb3",
    },
    Paper {
        file: "dyla-final.pdf",
        url: "https://bracha.org/dyla-final.pdf",
        sha256: "a4cd9130299397cb073efcf789b8fd5bf0fed268d926dee28cb3d8ed1101e5cb",
    },
    Paper {
        file: "mirrors.pdf",
        url: "https://bracha.org/mirrors.pdf",
        sha256: "a5d35871f26dbea062224ebbaf066e4325e8bff66e681ce8d64216a1ea3c5085",
    },
    Paper {
        file: "newspeak.pdf",
        url: "https://bracha.org/newspeak.pdf",
        sha256: "64579153930f68389e0728534e051c1da07a535d13d7090bec165ce44f6de9f6",
    },
    Paper {
        file: "objectsAsSoftwareServices.pdf",
        url: "https://bracha.org/objectsAsSoftwareServices.pdf",
        sha256: "fe9a138fbf8b919dea72980f2642e3ca4ff90a3654309c0b73831714a37e9000",
    },
    Paper {
        file: "newspeak-exemplars.pdf",
        url: "https://newspeaklanguage.org/pubs/newspeak-exemplars.pdf",
        sha256: "588f0d10e2cd02ceb008c9725989681ee42b3d36307ae0e2c54f0927ea30db4a",
    },
    Paper {
        file: "modularity-meets-inheritance.pdf",
        url: "https://bracha.org/modularity-meets-inheritance.pdf",
        sha256: "cfd1929a6c7183e2009979756d9178e620a98459c9dab29d589c19fcc4486e6b",
    },
];

/// The suffix of a download not yet checked, before it is renamed into
/// place, so that a refused or broken download never stands under the
/// name a document cites.
const PARTIAL_SUFFIX: &str = ".partial";

/// Downloads every paper missing from `docs/references/` under `root`, or
/// standing there with other bytes, and reports each.
pub fn fetch(root: &Path) -> ExitCode {
    let directory = root.join("docs").join("references");
    let mut failed = false;
    for paper in PAPERS {
        match fetch_one(&directory, paper) {
            Ok(Fetched::AlreadyThere) => println!("{}: already there", paper.file),
            Ok(Fetched::Downloaded) => println!("{}: downloaded", paper.file),
            Err(failure) => {
                eprintln!("{}: {failure}", paper.file);
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// What the task did for one paper.
#[derive(Debug, PartialEq)]
enum Fetched {
    AlreadyThere,
    Downloaded,
}

fn fetch_one(directory: &Path, paper: &Paper) -> Result<Fetched, String> {
    let target = directory.join(paper.file);
    if let Ok(bytes) = std::fs::read(&target)
        && sha256_hex(&bytes) == paper.sha256
    {
        return Ok(Fetched::AlreadyThere);
    }
    let partial = PathBuf::from(format!("{}{PARTIAL_SUFFIX}", target.display()));
    let status = Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--location"])
        .args(["--max-time", "120", "--output"])
        .arg(&partial)
        .arg(paper.url)
        .status()
        .map_err(|error| format!("curl could not be started: {error}"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("downloading {} failed ({status})", paper.url));
    }
    let bytes =
        std::fs::read(&partial).map_err(|error| format!("reading the download failed: {error}"))?;
    let actual = sha256_hex(&bytes);
    if actual != paper.sha256 {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "{} answered other bytes than the version the documents cite \
             (SHA-256 {actual}, expected {}); see docs/references/PROVENANCE.md, \
             \"Refreshing a paper\"",
            paper.url, paper.sha256
        ));
    }
    std::fs::rename(&partial, &target)
        .map_err(|error| format!("placing the download failed: {error}"))?;
    Ok(Fetched::Downloaded)
}

/// The SHA-256 round constants (FIPS 180-4, section 4.2.2).
const ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// The SHA-256 initial hash value (FIPS 180-4, section 5.3.3).
const INITIAL_HASH: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// The SHA-256 of `bytes` in lowercase hex. Written here rather than
/// taken from a crate: the task is the one place the repository hashes
/// a file this way, and FIPS 180-4 fits in a page.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut message = bytes.to_vec();
    let bit_length = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());
    let mut hash = INITIAL_HASH;
    for block in message.as_chunks::<64>().0 {
        let mut schedule = [0u32; 64];
        for (index, word) in block.as_chunks::<4>().0.iter().enumerate() {
            schedule[index] = u32::from_be_bytes(*word);
        }
        for index in 16..64 {
            let earlier = schedule[index - 15];
            let later = schedule[index - 2];
            let sigma0 = earlier.rotate_right(7) ^ earlier.rotate_right(18) ^ (earlier >> 3);
            let sigma1 = later.rotate_right(17) ^ later.rotate_right(19) ^ (later >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(sigma0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(sigma1);
        }
        let mut state = hash;
        for (constant, word) in ROUND_CONSTANTS.iter().zip(schedule) {
            let [first, second, third, fourth, fifth, sixth, seventh, eighth] = state;
            let sum1 = fifth.rotate_right(6) ^ fifth.rotate_right(11) ^ fifth.rotate_right(25);
            let choice = (fifth & sixth) ^ (!fifth & seventh);
            let temp1 = eighth
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(*constant)
                .wrapping_add(word);
            let sum0 = first.rotate_right(2) ^ first.rotate_right(13) ^ first.rotate_right(22);
            let majority = (first & second) ^ (first & third) ^ (second & third);
            let temp2 = sum0.wrapping_add(majority);
            state = [
                temp1.wrapping_add(temp2),
                first,
                second,
                third,
                fourth.wrapping_add(temp1),
                fifth,
                sixth,
                seventh,
            ];
        }
        for (word, added) in hash.iter_mut().zip(state) {
            *word = word.wrapping_add(added);
        }
    }
    hash.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_fips_180s() {
        // FIPS 180-4's examples, and one message whose padding takes a
        // second block.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn every_paper_is_ignored_and_listed_with_its_checksum() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let ignored = std::fs::read_to_string(root.join(".gitignore")).expect(".gitignore");
        let provenance = std::fs::read_to_string(root.join("docs/references/PROVENANCE.md"))
            .expect("PROVENANCE.md");
        for paper in PAPERS {
            let pattern = format!("/docs/references/{}", paper.file);
            assert!(
                ignored.lines().any(|line| line.trim() == pattern),
                ".gitignore does not ignore {}",
                paper.file
            );
            let row = provenance
                .lines()
                .find(|line| line.starts_with(&format!("| `{}`", paper.file)))
                .unwrap_or_else(|| panic!("PROVENANCE.md has no row for {}", paper.file));
            assert!(
                row.contains(paper.url) && row.contains(paper.sha256),
                "PROVENANCE.md's row for {} does not give {} and {}",
                paper.file,
                paper.url,
                paper.sha256
            );
        }
    }
}
