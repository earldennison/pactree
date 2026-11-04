# Third-party notices

Pactree software is licensed under MIT. The two legacy lexical data tables
below have separate terms; the MIT license does not replace those terms.
The tables support the legacy codec and conversion tools. PACTBIN2 storage
does not use them. Dictionary entries and their indices are unchanged here.

## English lexical data

`src/file_format/english_lexpack.rs` contains a selected and ordered Rust
string-table adaptation of English data from wordfreq 3.1.1.

Copyright 2022 Robyn Speer. The adapted data is distributed under
[Creative Commons Attribution-ShareAlike 4.0 International](https://creativecommons.org/licenses/by-sa/4.0/).
wordfreq's software license is Apache-2.0; its data has separate licensing.
See [wordfreq](https://github.com/rspeer/wordfreq), its
[attribution notice](https://github.com/rspeer/wordfreq/blob/master/NOTICE.md),
and its [licensing documentation](https://github.com/rspeer/wordfreq#license).
This adaptation is not endorsed by the original authors.

wordfreq credits the following data sources:

- [Google Books Ngrams](https://books.google.com/ngrams) and Google Books
  Syntactic Ngrams.
- [Leeds Internet Corpus](http://corpus.leeds.ac.uk/list.html), University of
  Leeds Centre for Translation Studies.
- [Wikipedia](https://www.wikipedia.org/) and [ParaCrawl](https://paracrawl.eu/).
- [OPUS OpenSubtitles 2018](https://opus.nlpl.eu/OpenSubtitles.php), originating
  from [OpenSubtitles](https://www.opensubtitles.org/).
- SUBTLEX-US, SUBTLEX-UK, SUBTLEX-CH, SUBTLEX-DE, and SUBTLEX-NL by Marc
  Brysbaert and collaborators. SUBTLEX is freely available data from the
  [Center for Reading Research](http://crr.ugent.be/programs-data/subtitle-frequencies).
  Relevant authors include Boris New; Matthias Buchmeier, Markus Conrad,
  Arthur M. Jacobs, Jens Bölte and Andrea Böhl; Qing Cai; Emmanuel Keuleers
  and Boris New for SUBTLEX-NL; and Walter J. B. van Heuven, Pawel Mandera,
  Emmanuel Keuleers and Marc Brysbaert for SUBTLEX-UK.

wordfreq also describes aggregate word-frequency statistics collected from
Twitter. The table is lexical data, not a collection of messages or posts.

## Shell lexical data

`src/file_format/shell_lexpack.rs` combines shell tokens with selected command
examples adapted from [tldr-pages](https://github.com/tldr-pages/tldr).

Copyright 2014-present the tldr-pages team and contributors. The adapted data
is distributed under [Creative Commons Attribution 4.0 International](https://creativecommons.org/licenses/by/4.0/).
See the [upstream license](https://github.com/tldr-pages/tldr/blob/main/LICENSE.md)
and [contributors](https://github.com/tldr-pages/tldr/graphs/contributors).
Selection, ordering, and conversion into a Rust string table are modifications
made for the codec. No endorsement by tldr-pages is implied.

## Dependencies

Cargo dependencies retain their individual licenses. `Cargo.lock` identifies
the exact dependencies used by this source release.
