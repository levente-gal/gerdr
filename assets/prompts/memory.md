The project's memory, where it exists: `GLOSSARY.md` holds its terms
(`GLOSSARY-MAP.md`, where there, points to more glossaries), and `docs/adr/`
holds its decisions that are hard to reverse, one file each. Look for them at
the top of each repository above{{#if multi_repo}}, and in the project's home
folder, the one that holds `.features/`{{/if}}. Read the glossary before you
name things, and use its terms in what you write and in the code; read the
decisions in the area you work in, and name one (ADR 0003) when your work
touches it or would go against it.
