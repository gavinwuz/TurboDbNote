// Comment-aware envelope; SQL spans retain the existing sequel highlights.
module.exports = grammar({
  name: 'mysql_comments',
  extras: $ => [],
  rules: {
    source_file: $ => repeat(choice($.comment, $.sql)),
    comment: _ => token(/#[^\r\n]*/),
    sql: _ => token(repeat1(choice(
      /[^#'"`\/-]+/,
      /'(?:[^'\\]|\\.|'')*'/,
      /"(?:[^"\\]|\\.|"")*"/,
      /`(?:[^`\\]|\\.|``)*`/,
      /--[^\r\n]*/,
      /\/\*[^*]*\*+(?:[^/*][^*]*\*+)*\//,
      /[\/-]/
    )))
  }
});
