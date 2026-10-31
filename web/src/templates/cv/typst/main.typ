// basic-resume template. Edit the details in resume.with, then the sections.
#import "@preview/basic-resume:0.2.10": *

#show: resume.with(
  author: "{{author}}",
  location: "Toronto, ON",
  email: "ada.obrien@example.com",
  github: "github.com/adaobrien",
  phone: "+1 (416) 555-0148",
  personal-site: "example.com",
  accent-color: "#26428b",
  font: "New Computer Modern",
  paper: "us-letter",
  author-position: left,
  personal-info-position: left,
)

// Lines that start with == become section headings.
== Education

#edu(
  institution: "University of Example",
  location: "Toronto, ON",
  dates: dates-helper(start-date: "Sep 2016", end-date: "Jun 2020"),
  degree: "Ph.D. in Economics",
)
- Dissertation on household credit and monetary policy

#edu(
  institution: "University of Example",
  location: "Toronto, ON",
  dates: dates-helper(start-date: "Sep 2014", end-date: "Jun 2016"),
  degree: "M.Sc. in Statistics, Distinction",
)

== Experience

#work(
  title: "Senior Data Scientist",
  location: "Toronto, ON",
  company: "Northwind Analytics",
  dates: dates-helper(start-date: "Jul 2021", end-date: "Present"),
)
- Lead a team of four building forecasting models for retail demand
- Cut weekly reporting time by 60 percent with a reproducible pipeline

#work(
  title: "Research Fellow",
  location: "Toronto, ON",
  company: "University of Example",
  dates: dates-helper(start-date: "Sep 2019", end-date: "Jun 2021"),
)
- Estimated panel models of consumer lending with administrative data
- Taught two undergraduate courses on applied regression

== Projects

#project(
  name: "Causal inference toolkit",
  role: "Maintainer",
  url: "example.com/causal",
  dates: dates-helper(start-date: "2023", end-date: "Present"),
)
- Python package for difference-in-differences estimators with over 2,000 downloads a month

== Publications

- Ada O'Brien and Robert Chen. Credit supply and rate pass-through. _Journal of Money, Credit and Banking_, 55(3), 411-438 (2023)
- Ada O'Brien. Measuring household debt with transaction data. _Review of Income Studies_, 12(1), 1-24 (2021)

== Skills

- *Programming*: Python, R, SQL, Stan
- *Tools*: Git, Docker, Quarto, LaTeX

== Languages

- *English*: Native
- *French*: Professional working proficiency
