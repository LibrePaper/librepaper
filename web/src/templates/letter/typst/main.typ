#let author = "{{author}}"
#let subject = "{{title}}"

#set document(title: subject)

#author
Your Address
City, State ZIP Code

*Date:* #datetime.today().display()

Recipient Name
Recipient Address
City, State ZIP Code

Dear Recipient,

#subject

I am writing to you regarding the matter discussed above. Please find details and context in the following paragraphs.

This letter serves as formal communication on the subject. I have carefully considered the relevant points and believe this is the appropriate course of action.

I look forward to your response. Please contact me if you have any questions or require additional information.

Sincerely,

#author
