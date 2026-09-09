data_path <- Sys.getenv("AUTONOMICS_INPUT0")
script_path <- Sys.getenv("AUTONOMICS_INPUT1")
output_path <- Sys.getenv("AUTONOMICS_OUTPUT0")
data_format <- Sys.getenv("AUTONOMICS_VISUALIZATION_DATA_FORMAT")
width <- as.numeric(Sys.getenv("AUTONOMICS_VISUALIZATION_WIDTH"))
height <- as.numeric(Sys.getenv("AUTONOMICS_VISUALIZATION_HEIGHT"))
dpi <- as.numeric(Sys.getenv("AUTONOMICS_VISUALIZATION_DPI"))

if (!nzchar(data_path) || !nzchar(script_path) || !nzchar(output_path)) {
  stop("visualization render requires input, script, and output bindings")
}

df <- switch(
  data_format,
  csv = arrow::read_csv_arrow(data_path, as_data_frame = TRUE),
  tsv = arrow::read_tsv_arrow(data_path, as_data_frame = TRUE),
  parquet = arrow::read_parquet(data_path, as_data_frame = TRUE),
  arrow_stream = arrow::read_ipc_stream(data_path, as_data_frame = TRUE),
  arrow_file = arrow::read_feather(data_path, as_data_frame = TRUE),
  stop("unsupported visualization data format: ", data_format)
)

source(script_path, local = FALSE)
if (!exists("p", inherits = TRUE) || is.null(p)) {
  stop("visualization script must assign a plot to a variable named `p`")
}

ggplot2::ggsave(
  filename = output_path,
  plot = p,
  device = "png",
  width = width,
  height = height,
  dpi = dpi,
  units = "in"
)
