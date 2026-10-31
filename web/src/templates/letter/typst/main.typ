// letterloom template. Edit the fields in the call below, then the body.
#import "@preview/letterloom:3.0.2": *

#show: letterloom.with(
  // Sender
  from-name: "{{author}}",
  from-address: [
    45 Maple Avenue \
    Toronto, ON M4C 2K1
  ],

  // Recipient
  to-name: "Dr. Jane Smith",
  to-address: [
    Department of Statistics \
    University of Example \
    123 College Street \
    Toronto, ON M5S 1A1
  ],

  // Date (today's date by default)
  date: datetime.today().display("[day padding:zero] [month repr:long] [year repr:full]"),

  // Subject line
  subject: "{{title}}",

  // Opening and closing
  salutation: "Dear Dr. Smith,",
  closing: "Sincerely,",

  // Signature block
  signatures: (
    (
      name: "{{author}}",
      affiliation: "Research Fellow, Department of Economics",
    ),
  ),

  // Carbon copy and enclosures
  cc: ("Prof. Robert Chen, Seminar Coordinator",),
  enclosures: (
    (description: "Curriculum vitae"),
    (description: "Seminar abstract"),
  ),
)

Thank you for your invitation to speak at the Spring Research Seminar on 11 March 2027. I am delighted to accept, and I am grateful for the opportunity to share our recent work with your department.

I have reserved the morning of 11 March for travel and will arrive the previous evening. Could you confirm the length of the session, whether a projector and a microphone will be available, and whether you prefer a particular format for the question period? I will send an abstract and slides one week before the event.

Please let me know if you need any other information for your records or the seminar program. I look forward to meeting you and your colleagues, and to the discussion that follows.

P.S. I would be glad to meet with graduate students during my visit if that would be useful.
