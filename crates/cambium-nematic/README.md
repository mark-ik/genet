# cambium-nematic

`cambium-nematic` provides reactive Cambium views and themes for smolweb
content parsed by Errand. It currently projects Gemtext, Gopher, RSS/Atom
feeds, and Nex directory listings.

The crate owns presentation only. Transport and protocol parsing live in
Errand, portable `EngineDocument` lowering lives in Nematic, and retained
layout and rendering sessions live in Genet's `genet-documents` component.

## License

MPL-2.0.
