# Fuel and water

[← Back to the README](../README.md)

Press `a` from the waiting dashboard to log one meal, a whole day of meals, or
today's plain-water intake.

## Meals and nutrition

Describe a single meal or paste a timeline containing several meals. Press
Enter to submit it. Svarog reviews a multi-meal description and saves each time
block as a separate meal.

Meal parsing uses `gpt-5.6-luna` through the selected Codex or OpenAI
recommender to estimate calories, protein, carbohydrates, fat, fiber, sugar,
sodium, and potassium. The Local recommender disables meal submission because
it does not parse natural-language food descriptions.

Natural-language dates are supported. Without a date, meals default to today.
An all-future multi-meal timeline entered before 04:00 local time is inferred as
yesterday.

Only the current description, local date and time, timezone, and unit-system
preference are sent when you explicitly submit a meal. Previous fuel entries
are never included. See [Data and privacy](data-and-privacy.md) for the broader
recommender data flow.

## Meals from photos

Attach up to 10 photos of what you ate. Svarog reads them with `gpt-5.6-luna`,
lists each meal's ingredients with estimated grams, and then sends that list
through the same text parser described above, so you review the result before
anything is saved.

- Press `Ctrl+V` in the meal box to attach a copied image. On macOS this also
  works for an image file copied in Finder. On Linux it needs `wl-paste`
  (Wayland) or `xclip` (X11).
- Drag image files into the terminal, or paste their paths, to attach them.
- Supported formats are PNG, JPEG, HEIC, WebP, and GIF, up to 20 MB each.
  Before sending, each photo is converted to a JPEG no larger than 1080 px on
  its long side, using `sips` on macOS or ImageMagick (or libheif for HEIC) on
  Linux. Without those tools, HEIC photos can't be sent and other photos are
  sent unchanged.
- Press Backspace while the text box is empty to remove the last photo.

Any text you type goes along as a note, for example `yesterday dinner, ate
half`, so dates, times, and portion hints still apply. Photos are sent to the
selected Codex or OpenAI backend only when you submit. The saved entry keeps
the generated text description, not the photos.

## Water

Plain-water tracking is always local. Use `+` or `=` and `-` to adjust today's
total by 200 ml or 8 US fluid ounces, depending on your selected unit system.

## Review your history

Select Fuel on the waiting dashboard and press Enter to open its statistics.
The view summarizes calories, macros, hydration, and weight trend using your
selected units.
