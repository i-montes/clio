# report

`total_by_category(path)` reads a CSV with the columns `date,category,amount,note` and returns a
dict `{category: Decimal}` with the sum of `amount` per category, each rounded to two decimals, rounding half up (2.005 becomes 2.01), as an invoice does.

- `amount` can be negative (a refund) and can have any number of decimals. Use `decimal.Decimal`,
  never `float`: 0.1 + 0.2 must give exactly 0.30.
- `note` may contain commas, quotes and even line breaks when it is quoted.
- A row with an empty `amount` is ignored.
- A file with only the header returns `{}`.
