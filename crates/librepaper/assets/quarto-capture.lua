-- Read the post-execution Pandoc AST. No source rewriting and no execution.
-- Only display results are exported; cell-code is used solely for association.
local destination = os.getenv("LIBREPAPER_QUARTO_CELL_MANIFEST")
local candidates = {}
local inline_records_path = os.getenv("LIBREPAPER_QUARTO_INLINE_RECORDS")
local inline_records = {}
local inline_values = {}

-- Quarto's execution phase usually replaces an inline expression with a
-- plain Pandoc string before this post-execution filter runs. The source
-- occurrence list lets us recover values only when the rendered paragraph
-- has an unambiguous prefix/suffix match. Ambiguous or formatted cases are
-- deliberately omitted; the native collector then keeps the source syntax.
local function load_inline_records()
  if not inline_records_path then return end
  local file = io.open(inline_records_path, "r")
  if not file then return end
  local encoded = file:read("*a")
  file:close()
  local ok, decoded = pcall(quarto.json.decode, encoded)
  if ok and type(decoded) == "table" then inline_records = decoded end
end

local function inline_parts(record)
  local line = record.source or ""
  local column = tonumber(record.column or 0) or 0
  if column < 2 then return nil end
  local opening = column - 1
  if line:sub(opening, opening) ~= "`" then return nil end
  local closing = line:find("`", column, true)
  if not closing then return nil end
  return line:sub(1, opening - 1), line:sub(closing + 1), closing
end

local function capture_inline_values(doc)
  local paragraphs = {}
  local records_per_line = {}
  for _, record in ipairs(inline_records) do
    records_per_line[record.line] = (records_per_line[record.line] or 0) + 1
  end
  local function paragraph(el)
    local codes = {}
    for _, inline in ipairs(el.content or {}) do
      if pandoc.utils.type(inline) == "Code" then codes[#codes + 1] = inline.text end
    end
    paragraphs[#paragraphs + 1] = {text = pandoc.utils.stringify(el), codes = codes}
  end
  doc:walk({
    Para = paragraph,
    Plain = paragraph
  })
  for _, record in ipairs(inline_records) do
    local prefix, suffix = inline_parts(record)
    if records_per_line[record.line] == 1 and prefix and suffix and prefix ~= "" and suffix ~= "" then
      local matches = {}
      for _, paragraph in ipairs(paragraphs) do
        local has_source_code = false
        for _, code in ipairs(paragraph.codes) do
          if code == record.expression then has_source_code = true end
        end
        if not has_source_code then
          local cursor = 1
          while true do
            local start = paragraph.text:find(prefix, cursor, true)
            local finish = start and paragraph.text:find(suffix, start + #prefix, true)
            if not (start and finish) then break end
            local value = paragraph.text:sub(start + #prefix, finish - 1)
            if value ~= record.expression then matches[#matches + 1] = value end
            cursor = finish + #suffix
          end
        end
      end
      if #matches == 1 then
        inline_values[#inline_values + 1] = {
          id = record.id,
          expression = record.expression,
          line = record.line,
          column = record.column,
          value = matches[1]
        }
      end
    end
  end
end
local function has(el, class)
  return el.classes and el.classes:includes(class)
end
local function escape(value)
  return tostring(value):gsub("&", "&amp;"):gsub("<", "&lt;"):gsub(">", "&gt;"):gsub('"', "&quot;")
end
local function table_html(el)
  local parts = {"<table>"}
  local function rows(values, header)
    for _, row in ipairs(values or {}) do
      parts[#parts + 1] = "<tr>"
      for _, cell in ipairs(row.cells or {}) do
        local tag = header and "th" or "td"
        parts[#parts + 1] = "<" .. tag .. ">" .. escape(pandoc.utils.stringify(cell.contents)) .. "</" .. tag .. ">"
      end
      parts[#parts + 1] = "</tr>"
    end
  end
  rows(el.head.rows, true)
  for _, body in ipairs(el.bodies) do rows(body.head, true); rows(body.body, false) end
  rows(el.foot.rows, false)
  parts[#parts + 1] = "</table>"
  return table.concat(parts)
end
local function capture(el)
  local item = {labels = {}, code = {}, outputs = {}}
  local seen = {}
  local function label(node)
    local id = node.identifier or ""
    if id ~= "" and not seen[id] then item.labels[#item.labels + 1] = id; seen[id] = true end
  end
  label(el)
  el:walk({
    Div = label,
    Image = function(img)
      label(img)
      item.outputs[#item.outputs + 1] = {kind = "image", asset = img.src, caption = pandoc.utils.stringify(img.caption)}
    end,
    Table = function(tbl)
      label(tbl)
      item.outputs[#item.outputs + 1] = {kind = "table", text = table_html(tbl)}
    end,
    CodeBlock = function(code)
      if has(code, "cell-code") then
        item.code[#item.code + 1] = code.text
      else
        item.outputs[#item.outputs + 1] = {kind = "text", text = code.text}
      end
    end
  })
  if #item.outputs > 0 or #item.code > 0 or #item.labels > 0 then candidates[#candidates + 1] = item end
end
function Pandoc(doc)
  load_inline_records()
  capture_inline_values(doc)
  doc:walk({
    Div = function(el)
      if has(el, "cell") or has(el, "quarto-float") then capture(el) end
    end
  })
  if destination then
    local file = assert(io.open(destination, "w"))
    file:write(quarto.json.encode({schema = 1, candidates = candidates, inline = inline_values}))
    file:close()
  end
  return doc
end
