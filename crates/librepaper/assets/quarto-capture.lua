-- Read the post-execution Pandoc AST. No source rewriting and no execution.
-- Only display results are exported; cell-code is used solely for association.
local destination = os.getenv("LIBREPAPER_QUARTO_CELL_MANIFEST")
local candidates = {}
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
  doc:walk({
    Div = function(el)
      if has(el, "cell") or has(el, "quarto-float") then capture(el) end
    end
  })
  if destination then
    local file = assert(io.open(destination, "w"))
    file:write(quarto.json.encode({schema = 1, candidates = candidates}))
    file:close()
  end
  return doc
end
