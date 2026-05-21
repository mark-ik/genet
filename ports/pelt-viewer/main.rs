/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Standalone entrypoint for the Pelt viewer. `pelt` itself launches the
//! same `pelt_viewer::run` through its `--engine viewer` path.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    pelt_viewer::run(std::env::args().nth(1))
}
