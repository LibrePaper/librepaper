#set page(header: [Long paper], footer: [#context counter(page).display()])
= Long paper fixture

#for section in range(1, 24) [
  == Section #section
  This repeated paragraph keeps enough content to exercise pagination and
  text extraction across multiple pages. It contains Unicode: Montréal, β,
  and 漢字. The generated PDF must retain selectable text and page breaks.
]
