//! The ggml model files nastro knows how to download, each pinned to an exact size and SHA-256.
//!
//! Generated from the Hugging Face API for `ggerganov/whisper.cpp` at [REPO_COMMIT] (the SHA-256 is the
//! file's Git LFS object id). Pinning the commit means a model's bytes cannot change underneath us, and the
//! hash means a corrupted or tampered download is rejected rather than loaded.

/// The `ggerganov/whisper.cpp` commit the table below was taken from.
pub const REPO_COMMIT: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

/// One downloadable ggml model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    /// Name as used in config (`whisper_model`), e.g. `large-v3-turbo` (file `ggml-<name>.bin`).
    pub name: &'static str,
    /// Exact size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 of the file.
    pub sha256: &'static str,
}

/// Every known model, sorted by name.
pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        name: "base-q5_1",
        size: 59707625,
        sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
    },
    ModelInfo {
        name: "base-q8_0",
        size: 81768585,
        sha256: "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9",
    },
    ModelInfo {
        name: "base",
        size: 147951465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
    },
    ModelInfo {
        name: "base.en-q5_1",
        size: 59721011,
        sha256: "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f",
    },
    ModelInfo {
        name: "base.en-q8_0",
        size: 81781811,
        sha256: "a4d4a0768075e13cfd7e19df3ae2dbc4a68d37d36a7dad45e8410c9a34f8c87e",
    },
    ModelInfo {
        name: "base.en",
        size: 147964211,
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
    },
    ModelInfo {
        name: "large-v1",
        size: 3094623691,
        sha256: "7d99f41a10525d0206bddadd86760181fa920438b6b33237e3118ff6c83bb53d",
    },
    ModelInfo {
        name: "large-v2-q5_0",
        size: 1080732091,
        sha256: "3a214837221e4530dbc1fe8d734f302af393eb30bd0ed046042ebf4baf70f6f2",
    },
    ModelInfo {
        name: "large-v2-q8_0",
        size: 1656129691,
        sha256: "fef54e6d898246a65c8285bfa83bd1807e27fadf54d5d4e81754c47634737e8c",
    },
    ModelInfo {
        name: "large-v2",
        size: 3094623691,
        sha256: "9a423fe4d40c82774b6af34115b8b935f34152246eb19e80e376071d3f999487",
    },
    ModelInfo {
        name: "large-v3-q5_0",
        size: 1081140203,
        sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
    },
    ModelInfo {
        name: "large-v3-turbo-q5_0",
        size: 574041195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
    ModelInfo {
        name: "large-v3-turbo-q8_0",
        size: 874188075,
        sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
    },
    ModelInfo {
        name: "large-v3-turbo",
        size: 1624555275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    },
    ModelInfo {
        name: "large-v3",
        size: 3095033483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    },
    ModelInfo {
        name: "medium-q5_0",
        size: 539212467,
        sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
    },
    ModelInfo {
        name: "medium-q8_0",
        size: 823369779,
        sha256: "42a1ffcbe4167d224232443396968db4d02d4e8e87e213d3ee2e03095dea6502",
    },
    ModelInfo {
        name: "medium",
        size: 1533763059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
    },
    ModelInfo {
        name: "medium.en-q5_0",
        size: 539225533,
        sha256: "76733e26ad8fe1c7a5bf7531a9d41917b2adc0f20f2e4f5531688a8c6cd88eb0",
    },
    ModelInfo {
        name: "medium.en-q8_0",
        size: 823382461,
        sha256: "43fa2cd084de5a04399a896a9a7a786064e221365c01700cea4666005218f11c",
    },
    ModelInfo {
        name: "medium.en",
        size: 1533774781,
        sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
    },
    ModelInfo {
        name: "small-q5_1",
        size: 190085487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
    },
    ModelInfo {
        name: "small-q8_0",
        size: 264464607,
        sha256: "49c8fb02b65e6049d5fa6c04f81f53b867b5ec9540406812c643f177317f779f",
    },
    ModelInfo {
        name: "small",
        size: 487601967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    },
    ModelInfo {
        name: "small.en-q5_1",
        size: 190098681,
        sha256: "bfdff4894dcb76bbf647d56263ea2a96645423f1669176f4844a1bf8e478ad30",
    },
    ModelInfo {
        name: "small.en-q8_0",
        size: 264477561,
        sha256: "67a179f608ea6114bd3fdb9060e762b588a3fb3bd00c4387971be4d177958067",
    },
    ModelInfo {
        name: "small.en",
        size: 487614201,
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
    },
    ModelInfo {
        name: "tiny-q5_1",
        size: 32152673,
        sha256: "818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7",
    },
    ModelInfo {
        name: "tiny-q8_0",
        size: 43537433,
        sha256: "c2085835d3f50733e2ff6e4b41ae8a2b8d8110461e18821b09a15c40c42d1cca",
    },
    ModelInfo {
        name: "tiny",
        size: 77691713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
    },
    ModelInfo {
        name: "tiny.en-q5_1",
        size: 32166155,
        sha256: "c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b",
    },
    ModelInfo {
        name: "tiny.en-q8_0",
        size: 43550795,
        sha256: "5bc2b3860aa151a4c6e7bb095e1fcce7cf12c7b020ca08dcec0c6d018bb7dd94",
    },
    ModelInfo {
        name: "tiny.en",
        size: 77704715,
        sha256: "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f",
    },
];
