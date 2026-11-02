export function gitLineNumberDigits(file) {
  let digits = 1;
  for (const hunk of file.hunks ?? []) {
    for (const line of hunk.lines ?? []) digits = Math.max(digits, String(line.oldLine ?? "").length, String(line.newLine ?? "").length);
  }
  return digits;
}
