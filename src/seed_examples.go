package main

// The example documents and the annotations seeded onto them. There is one
// document per source format Komodoc accepts, and each is genuinely produced by
// the tool it is named after: Quarto renders the .qmd, Calepin the .typ, marimo
// exports its own notebook, nbconvert executes and exports the Jupyter one,
// Komodoc's own markdown renderer handles the .md at publish time, and the HTML
// example is hand-written and rendered by nothing at all. The Makefile holds
// the commands.
//
// Between them the annotations use every kind: a plain comment, a question, a
// bare highlight with no words at all, a suggested edit carrying replacement
// text, a judgement, and a rectangle drawn on a figure. Several carry tags,
// some have replies, and one is already resolved, so the sidebar shows what
// each state looks like.
//
// Every Exact below has to appear in the rendered HTML. `seed` says so when one
// does not, rather than writing an annotation that anchors nowhere.

var seedDocuments = []seedDocument{
	{
		// Hand-written HTML: no stylesheet, no script, no build step. The
		// plainest thing Komodoc can host, and a contrast with the rest.
		File:  "examples/style-guide.html",
		Title: "HTML: A Short Style Guide for Quantitative Writing",
		Annotations: []seedAnnotation{
			{
				Motivation: "commenting",
				Exact:      "A number in a sentence is being read, not computed.",
				Body:       "Worth promoting to the top of the section. It is the reason for every rule under it.",
				Tags:       []string{"framing"},
				Creator:    "Vincent",
			},
			{
				Motivation: "questioning",
				Exact:      "the commonest error in this genre",
				Body:       "Commonest by what count? If there is a source for this, cite it; if it is an impression, say so.",
				Tags:       []string{"evidence"},
				Creator:    "Reviewer",
				Replies:    []string{"It is an impression. I will soften it to \"a common error\"."},
			},
			{
				Motivation: "highlighting",
				Exact:      "A figure that could have been a sentence should be a sentence.",
				Creator:    "Reviewer",
				Tags:       []string{"teaching"},
			},
			{
				Motivation:  "editing",
				Exact:       "Alphabetical order is meaningful only for looking things up.",
				Body:        "True, but it reads as a throwaway. Give it the weight it deserves.",
				Replacement: "Alphabetical order is meaningful only when the reader arrives knowing which row they want.",
				Tags:        []string{"style"},
				Creator:     "Vincent",
			},
			{
				Motivation: "assessing",
				Exact:      "it has a technical meaning and an ordinary one",
				Body:       "This is the strongest paragraph in the guide and it is buried in a table's aftermath. It should be its own section.",
				Tags:       []string{"structure"},
				Creator:    "Vincent",
				Resolved:   true,
			},
			{
				Motivation: "commenting",
				Exact:      "Right-align numbers, left-align text",
				Body:       "The table above does not follow its own advice: the estimate column is right-aligned, but the header is not.",
				Tags:       []string{"tables", "accuracy"},
				Creator:    "Reviewer",
			},
		},
	},
	{
		// Markdown, rendered by Komodoc itself on publication: the file that is
		// read here is the .md, and readSeedDocument runs it through the same
		// renderer an upload would.
		File:  "examples/regression-tables.md",
		Title: "Markdown: What a Regression Table Is Hiding",
		Annotations: []seedAnnotation{
			{
				Motivation: "commenting",
				Exact:      "every summary is a decision about what to leave out",
				Body:       "This is the thesis, and it arrives in the first sentence. Good.",
				Tags:       []string{"framing"},
				Creator:    "Vincent",
			},
			{
				Motivation: "questioning",
				Exact:      "A model fit on 4,102 of 11,000 rows is a model of the 4,102.",
				Body:       "Is the 11,000 a real figure or an illustration? If it is illustrative, say so, because it reads as a specific study.",
				Tags:       []string{"evidence"},
				Creator:    "Reviewer",
				Replies: []string{
					"Illustrative. I will make the numbers obviously round.",
				},
			},
			{
				Motivation: "highlighting",
				Exact:      "A tight interval around a biased estimate is the most misleading object in applied statistics",
				Creator:    "Reviewer",
				Tags:       []string{"teaching"},
			},
			{
				Motivation:  "editing",
				Exact:       "Standard errors clustered at the wrong level are not conservative; they are simply wrong, and usually too small.",
				Body:        "Two claims in one sentence, and the second is the surprising one. Split them.",
				Replacement: "Standard errors clustered at the wrong level are not conservative. They are wrong, and usually too small.",
				Tags:        []string{"style"},
				Creator:     "Vincent",
			},
			{
				Motivation: "assessing",
				Exact:      "An effect that appears in the pooled data and in neither half is not a subtle effect.",
				Body:       "The most useful sentence in the note, and it is third in a numbered list where nobody will find it.",
				Tags:       []string{"structure"},
				Creator:    "Vincent",
				Resolved:   true,
			},
			{
				Motivation: "commenting",
				Exact:      "A table that admits nothing is not a table without problems.",
				Body:       "A good closing line. It would be stronger still if the note gave one real example of a table doing this well.",
				Tags:       []string{"exposition"},
				Creator:    "Reviewer",
			},
		},
	},
	{
		File:  "examples/bootstrap.html",
		Title: "Quarto: What the Bootstrap Actually Resamples",
		Annotations: []seedAnnotation{
			{
				Motivation: "commenting",
				Exact:      "The approximation is the whole method",
				Body:       "This is the sentence the rest of the note hangs on. Worth putting it in the abstract too.",
				Tags:       []string{"framing"},
				Creator:    "Vincent",
				Replies: []string{
					"Agreed. I would go further and say it belongs in the first line.",
				},
			},
			{
				Motivation: "questioning",
				Exact:      "The bootstrap says nothing about that gap",
				Body:       "Is that strictly true? A bootstrap bias estimate exists, even if it is noisy. Perhaps: says nothing about that gap without further assumptions?",
				Tags:       []string{"accuracy", "bias"},
				Creator:    "Reviewer",
			},
			{
				Motivation: "highlighting",
				Exact:      "the bootstrap distribution of the maximum is degenerate at the top",
				Creator:    "Vincent",
				Tags:       []string{"teaching"},
			},
			{
				Motivation:  "editing",
				Exact:       "The interval is not wrong so much as over-confident",
				Body:        "Sharper, and avoids implying intent.",
				Replacement: "The interval is not wrong; it is too narrow.",
				Tags:        []string{"style"},
				Creator:     "Reviewer",
			},
			{
				Motivation: "assessing",
				Exact:      "no number of bootstrap replicates",
				Body:       "This is the most useful paragraph in the note. It is also the one most readers will skip, because it arrives after the plot.",
				Creator:    "Vincent",
				Resolved:   true,
				Replies:    []string{"Moved it above the figure in the next draft."},
			},
			{
				// The first figure: the two densities, with the offset between
				// them that the text is about.
				Motivation: "commenting",
				Body:       "The offset between the two peaks is the point of the figure, but nothing in the image says so. A short arrow and a label would carry it.",
				Tags:       []string{"figures"},
				Creator:    "Reviewer",
				Region:     &region{ImageIndex: 0, X: 34, Y: 12, Width: 30, Height: 62},
			},
		},
	},
	{
		File:  "examples/newton.html",
		Title: "Calepin: Newton's Method Is Not Always Your Friend",
		Annotations: []seedAnnotation{
			{
				Motivation: "commenting",
				Exact:      "the qualification everyone forgets",
				Body:       "Good opening. It states the thesis in the first sentence and the rest of the note earns it.",
				Tags:       []string{"framing"},
				Creator:    "Vincent",
			},
			{
				Motivation: "questioning",
				Exact:      "for some",
				Body:       "Should this say where the intermediate point comes from? A reader who has not seen Taylor's theorem with remainder will not know why such a point exists.",
				Tags:       []string{"exposition", "proofs"},
				Creator:    "Reviewer",
				Replies: []string{
					"Fair. One clause about the mean value form would cover it.",
					"Added a footnote rather than a clause, to keep the line short.",
				},
			},
			{
				Motivation: "highlighting",
				Exact:      "The method is not lost, and it is not diverging.",
				Creator:    "Vincent",
			},
			{
				Motivation:  "editing",
				Exact:       "Newton is a local method wearing a global disguise.",
				Body:        "Lovely line, but it lands better without the metaphor doing double duty.",
				Replacement: "Newton's method is local, and nothing about its statement says so.",
				Tags:        []string{"style"},
				Creator:     "Reviewer",
			},
			{
				Motivation: "assessing",
				Exact:      "Two initial guesses agreeing to three decimals can land on different roots.",
				Body:       "This is the claim a sceptical reader will want checked. The figure supports it, but the text should give the two values explicitly.",
				Tags:       []string{"evidence"},
				Creator:    "Reviewer",
			},
			{
				// The convergence plot: the gap between the two curves.
				Motivation: "commenting",
				Body:       "Consider marking where the blue curve hits machine precision. The flat tail is an artefact of double precision, not of the method, and it reads as convergence stalling.",
				Tags:       []string{"figures"},
				Creator:    "Vincent",
				Region:     &region{ImageIndex: 0, X: 55, Y: 60, Width: 40, Height: 32},
			},
		},
	},
	{
		File:  "examples/simpsons-paradox.html",
		Title: "Jupyter: Simpson's Paradox Is Not a Paradox",
		Annotations: []seedAnnotation{
			{
				Motivation: "commenting",
				Exact:      "the arithmetic is not in dispute and both lines are correct",
				Body:       "This is the right framing. Most treatments present the reversal as an error to be caught rather than as two answers to two questions.",
				Tags:       []string{"framing"},
				Creator:    "Vincent",
			},
			{
				Motivation: "questioning",
				Exact:      "The slope is positive and it is not a rounding error.",
				Body:       "Worth giving the standard error here. A reader who suspects the whole thing is noise will not be persuaded by the point estimate alone.",
				Tags:       []string{"evidence"},
				Creator:    "Reviewer",
				Replies: []string{
					"Added it to the printed output rather than the prose.",
				},
			},
			{
				Motivation: "highlighting",
				Exact:      "Both departments slope down. The pooled line slopes up.",
				Creator:    "Reviewer",
				Tags:       []string{"teaching"},
			},
			{
				Motivation:  "editing",
				Exact:       "the pooled line reads that coincidence as a causal slope",
				Body:        "\"Coincidence\" undersells it: the confounding is structural, not accidental.",
				Replacement: "the pooled line reads that difference as a causal slope",
				Tags:        []string{"style", "accuracy"},
				Creator:     "Vincent",
			},
			{
				Motivation: "assessing",
				Exact:      "no amount of staring at the scatterplot will answer it",
				Body:       "This is the paragraph that earns the notebook. It should arrive before the figures, not after them.",
				Tags:       []string{"structure"},
				Creator:    "Vincent",
				Resolved:   true,
			},
			{
				// The second figure: the same points split by department, where
				// the two within-group lines disagree with the dashed pooled one.
				Motivation: "questioning",
				Body:       "Could the dashed pooled line be drawn only across the gap between the two clouds? Running it through both groups is what makes it look like a fit to each.",
				Tags:       []string{"figures"},
				Creator:    "Reviewer",
				Region:     &region{ImageIndex: 1, X: 10, Y: 8, Width: 80, Height: 60},
			},
		},
	},
	{
		// marimo's own HTML export, unmodified. It loads the marimo frontend
		// from a CDN and keeps the prose in a JSON island that only its
		// JavaScript hydrates, so visibleText finds nothing to anchor against
		// and this example carries no seeded annotations. A reader can still
		// annotate it by hand once the page has rendered. It is here as what
		// marimo actually produces rather than as something rewritten to suit
		// the seeder.
		File:        "examples/random-walks.html",
		Title:       "Marimo: How Far Does a Drunk Walk?",
		Annotations: nil,
	},
}
