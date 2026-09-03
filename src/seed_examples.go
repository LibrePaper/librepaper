package main

// The example documents and the annotations seeded onto them. Between them
// they use every kind: a plain comment, a question, a bare highlight with no
// words at all, a suggested edit carrying replacement text, a judgement, and a
// rectangle drawn on a figure. Several carry tags, some have replies, and one
// is already resolved, so the sidebar shows what each state looks like.
//
// Every Exact below has to appear in the rendered HTML. `seed` says so when one
// does not, rather than writing an annotation that anchors nowhere.

var seedDocuments = []seedDocument{
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
		// Plain Typst: headings, lists and tables, and no styling of its own.
		// A useful contrast with the other three, which arrive dressed.
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
		File:  "examples/random-walks.html",
		Title: "Marimo: How Far Does a Random Walk Go?",
		Annotations: []seedAnnotation{
			{
				Motivation: "commenting",
				Exact:      "That fact is true and almost useless",
				Body:       "Exactly right, and worth saying this bluntly. Most treatments open with the mean and never explain why it tells you nothing.",
				Tags:       []string{"framing"},
				Creator:    "Vincent",
				Replies:    []string{"It is my favourite sentence in the note."},
			},
			{
				Motivation: "questioning",
				Exact:      "Four times as many steps take you only twice as far.",
				Body:       "Is it worth noting that this is why diffusion is slow at large scales? One sentence would connect it to something physical.",
				Tags:       []string{"exposition"},
				Creator:    "Reviewer",
			},
			{
				Motivation: "highlighting",
				Exact:      "The least likely outcome is an even split.",
				Creator:    "Reviewer",
				Tags:       []string{"teaching"},
			},
			{
				Motivation:  "editing",
				Exact:       "In a season of coin flips, one team leading throughout is not evidence of anything.",
				Body:        "The analogy is doing a lot of work in one line. Spell out the transfer.",
				Replacement: "A team that leads a season of coin flips from start to finish is not thereby a better team.",
				Tags:        []string{"style", "exposition"},
				Creator:     "Vincent",
			},
			{
				Motivation: "assessing",
				Exact:      "Reporting the mean of this sample would be reporting a property of the cap.",
				Body:       "This is correct and it is the sort of error that appears in published simulation studies. It deserves more than a closing sentence.",
				Tags:       []string{"evidence", "methods"},
				Creator:    "Reviewer",
				Resolved:   true,
			},
			{
				// The twenty paths with the sqrt envelope.
				Motivation: "questioning",
				Body:       "How many of the twenty paths leave the envelope here? Counting them would make the point that the envelope is a typical scale, not a bound.",
				Tags:       []string{"figures"},
				Creator:    "Vincent",
				Region:     &region{ImageIndex: 1, X: 8, Y: 6, Width: 84, Height: 26},
			},
		},
	},
	{
		File:  "examples/bootstrap-jupyter.html",
		Title: "Jupyter: Bootstrap Sampling and Confidence Intervals",
		Annotations: []seedAnnotation{
			{
				Motivation: "commenting",
				Exact:      "Bootstrap is a resampling method",
				Body:       "This is a clear and direct opening. It establishes what we are learning about.",
				Tags:       []string{"framing"},
				Creator:    "Vincent",
			},
			{
				Motivation: "questioning",
				Exact:      "assumes that the observed sample is representative",
				Body:       "When would this assumption fail? Are there cases where bootstrap is not appropriate?",
				Tags:       []string{"exposition"},
				Creator:    "Reviewer",
			},
			{
				Motivation: "highlighting",
				Exact:      "sampling variability",
				Creator:    "Vincent",
				Tags:       []string{"key-concept"},
			},
		},
	},
}
