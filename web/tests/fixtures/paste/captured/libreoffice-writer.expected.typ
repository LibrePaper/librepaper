Tidewater grant application

Lead investigator: Gérard Côté\
Tidewater Lab, 12 Quay Road\
Port Mersey, Nova Scotia

= What are you working on?

_Describe the problem in two or three sentences. Plain language is fine._

Tidewater is a tool that maps river sediment. It turns core samples, depth readings and historical surveys into a map of where silt, sand and clay have moved over the last fifty years. The project began as a pilot in 2025 — small as it was, the pilot showed that the method works on a real estuary.

Our *main claim* is simple: sediment maps can be cheap. Most _existing_ surveys cost *_far more_* than a town can afford, and the data often sits unused in `csv` files or in filing cabinets. We use `tw_map()` to turn raw depth logs into a layered map that a council engineer can read.

Read more about #link("https://example.org/pilot")[the pilot]. The full data policy is at #link("https://example.org/").

The budget request is \$150,000 over two years. Field costs run about \~5m per site, and roughly \~40% of the budget goes to laboratory work. Software licences are offered at 50 % off for public bodies.

Column names use snake\_case throughout. Templates use {braces} for placeholders, and on Windows the data lives under C:\\path\\tidewater. The model is quadratic in depth, so the fit includes an x^2 term. See \@ref for the sampling protocol, and wrap values in \<tag> when exporting HTML.

Our R&D partners will review the design. Their comments are welcome — the more critical, the better — and we have built them into the schedule.

The plan is in our own words, not “borrowed” from a funder’s template, and the team’s plan has been checked by two outside readers.

+ Collect cores from three sites.
+ Label each sample by depth and date.
+ Run the mapping pipeline.
  - Check the depth readings against the survey.
  - Flag any core older than ten years.
+ Publish the maps under an open licence.
+ Train two council staff to run the tool.

Items 1-3 are the technical core, and items 4 and 5 are outreach. If \#1 slips, the rest of the schedule slips with it.

== What would you do with funding?

_Be specific about what the money buys._

Funding pays for three kinds of work. Each one maps onto a line of the budget.

- Field work
  + Collect cores at each site
    + Label samples on site
  + Survey the channel by boat
- Lab work
- Software

#table(
  columns: 3,
  table.header([Site], [Depth (m)], [Sample]),
  [Lower Tidewater], [4.5], [Silt],
  [Mill Creek], [2.0], [Sand],
)

#quote(block: true)[
Sediment records where the river has been. Tidewater shows where it is going.
]

A sample analysis script looks like this:

```
library(tidewater)
cores <- read_cores("data/cores.csv")
map <- tw_map(cores, layer = "silt")
```

Thank you for reading. We look forward to hearing from you.
