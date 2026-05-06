/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#[cfg(feature = "viewer-engine")]
mod viewer;

pub const VERSION: &str = concat!("Pelt ", env!("CARGO_PKG_VERSION"));

pub fn main() {
    #[cfg(feature = "viewer-engine")]
    viewer::main();

    #[cfg(not(feature = "viewer-engine"))]
    {
        eprintln!(
            "pelt was built without an engine feature; enable viewer-engine or viewer-netrender"
        );
        std::process::exit(2);
    }
}
