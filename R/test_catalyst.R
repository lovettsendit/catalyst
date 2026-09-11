# Checks for Catalyst's own R side. Base R, zero-argument `test_*` functions,
# run by the host's R lane.
#
# The point of most of these is not that the checker passes a good export --
# that is easy to arrange by accident -- but that it *fails* a bad one. A
# checker that cannot fail is not a checker, so the exports built here are
# built deliberately broken, one way at a time.

catalyst_source_helpers <- function() {
  for (candidate in c("catalyst.R", file.path("R", "catalyst.R"))) {
    if (file.exists(candidate)) {
      source(candidate)
      return(invisible(candidate))
    }
  }
  stop("catalyst.R was not found relative to this working directory")
}

catalyst_source_helpers()

# A tiny export, written the way `catalyst export r` writes one: f(x) = x * x
# on [0, 10], with two points inside the domain and one below it.
#
# Small enough to read in one screen, which matters -- if the fixture these
# checks are built on were itself hard to verify, a failure here would not say
# anything.
catalyst_write_example_export <- function(dir) {
  dir.create(dir, recursive = TRUE, showWarnings = FALSE)
  writeLines(c(
    "catalyst_input_names <- c(\"x\")",
    "catalyst_input_min <- c(0)",
    "catalyst_input_max <- c(10)",
    "catalyst_in_domain <- function(p) {",
    "  for (i in seq_along(catalyst_input_names)) {",
    "    name <- catalyst_input_names[[i]]",
    "    if (is.null(p[[name]])) {",
    "      return(list(ok = FALSE, why = paste0(name, \" was not given a value\")))",
    "    }",
    "    if (p[[name]] < catalyst_input_min[[i]]) {",
    "      return(list(ok = FALSE, why = paste0(name, \" is below its declared minimum\")))",
    "    }",
    "    if (p[[name]] > catalyst_input_max[[i]]) {",
    "      return(list(ok = FALSE, why = paste0(name, \" is above its declared maximum\")))",
    "    }",
    "  }",
    "  list(ok = TRUE, why = \"\")",
    "}",
    "catalyst_value <- function(p) p$x * p$x",
    "catalyst_gradient <- function(p) list(x = 2 * p$x)"
  ), file.path(dir, "function.R"))
  writeLines(
    "catalyst_parameters <- list(list(name = \"x\", value = 2, min = 0, max = 10, unit = \"\"))",
    file.path(dir, "parameters.R"))
  writeLines(c(
    "catalyst_tolerance <- list(relative = 1e-9, absolute = 1e-12)",
    "catalyst_fixtures <- list(",
    "  list(name = \"a\", kind = \"normal\", inputs = list(x = 2),",
    "       refused = FALSE, nonfinite = FALSE, value = 4, gradient = list(x = 4)),",
    "  list(name = \"b\", kind = \"normal\", inputs = list(x = 3),",
    "       refused = FALSE, nonfinite = FALSE, value = 9, gradient = list(x = 6)),",
    "  list(name = \"outside\", kind = \"out_of_domain\", inputs = list(x = -1),",
    "       refused = TRUE, nonfinite = FALSE)",
    ")"
  ), file.path(dir, "fixtures.R"))
  invisible(dir)
}

# A fresh directory nobody else is using, removed when the caller is done.
catalyst_with_example_export <- function(body) {
  dir <- tempfile(pattern = "catalyst-r-check-")
  on.exit(unlink(dir, recursive = TRUE), add = TRUE)
  catalyst_write_example_export(dir)
  body(dir)
}

# The message of the error `expression` raises, or NULL if it raises none.
catalyst_error_message <- function(expression) {
  tryCatch({
    force(expression)
    NULL
  }, error = function(e) conditionMessage(e))
}

catalyst_edit_fixtures <- function(dir, from, to) {
  path <- file.path(dir, "fixtures.R")
  text <- readLines(path)
  hit <- FALSE
  for (i in seq_along(text)) {
    if (!hit && grepl(from, text[[i]], fixed = TRUE)) {
      text[[i]] <- sub(from, to, text[[i]], fixed = TRUE)
      hit <- TRUE
    }
  }
  if (!hit) {
    stop(sprintf("nothing to edit: no line of fixtures.R contains %s", from))
  }
  writeLines(text, path)
  invisible(path)
}

test_catalyst_close_is_the_declared_tolerance_rule <- function() {
  rel <- 1e-9
  tol <- 1e-12
  if (!catalyst_close(1, 1, rel, tol)) {
    stop("a number must be close to itself")
  }
  # Just inside the relative bound, and just outside it.
  if (!catalyst_close(1e6, 1e6 + 1e-4, rel, tol)) {
    stop("1e-10 of relative difference is inside a 1e-9 relative tolerance")
  }
  if (catalyst_close(1e6, 1e6 + 1, rel, tol)) {
    stop("1e-6 of relative difference is outside a 1e-9 relative tolerance")
  }
  # Near zero the absolute bound is the one that decides.
  if (!catalyst_close(0, 1e-13, rel, tol)) {
    stop("1e-13 is inside a 1e-12 absolute tolerance")
  }
  if (catalyst_close(0, 1e-9, rel, tol)) {
    stop("1e-9 is outside a 1e-12 absolute tolerance")
  }
  # Non-finite values compare exactly, and NaN matches NaN -- otherwise a
  # fixture that says "there is no number here" could never pass.
  if (!catalyst_close(NaN, NaN, rel, tol)) {
    stop("a fixture with no number must match a run with no number")
  }
  if (!catalyst_close(Inf, Inf, rel, tol)) {
    stop("an infinity must match itself")
  }
  if (catalyst_close(Inf, 1e300, rel, tol)) {
    stop("an infinity must not match a finite number")
  }
  invisible(TRUE)
}

test_catalyst_check_export_checks_every_case <- function() {
  catalyst_with_example_export(function(dir) {
    checked <- catalyst_check_export(dir)
    if (checked != 3L) {
      stop(sprintf("the example export has three cases, and the checker counted %d", checked))
    }
  })
  invisible(TRUE)
}

test_a_corrupted_fixture_value_is_refused_rather_than_passed <- function() {
  catalyst_with_example_export(function(dir) {
    catalyst_edit_fixtures(dir, "value = 4", "value = 1e9 + 4")
    message <- catalyst_error_message(catalyst_check_export(dir))
    if (is.null(message)) {
      stop("a fixture whose expected value is wrong by 1e9 was accepted")
    }
    if (!grepl("case", message, fixed = TRUE)) {
      stop(sprintf("the refusal must name the case that disagreed, it said: %s", message))
    }
    if (!grepl("case a:", message, fixed = TRUE)) {
      stop(sprintf("the refusal must name the case `a` that disagreed, it said: %s", message))
    }
  })
  invisible(TRUE)
}

test_a_corrupted_fixture_gradient_is_refused_rather_than_passed <- function() {
  catalyst_with_example_export(function(dir) {
    catalyst_edit_fixtures(dir, "gradient = list(x = 4)", "gradient = list(x = 40)")
    message <- catalyst_error_message(catalyst_check_export(dir))
    if (is.null(message)) {
      stop("a fixture whose expected partial is wrong by a factor of ten was accepted")
    }
    if (!grepl("case", message, fixed = TRUE)) {
      stop(sprintf("the refusal must name the case that disagreed, it said: %s", message))
    }
  })
  invisible(TRUE)
}

test_a_fixture_that_disagrees_about_the_domain_is_refused <- function() {
  catalyst_with_example_export(function(dir) {
    # The point is outside the declared range; claiming it is not is the
    # mistake an export makes when its domain check and its fixtures were
    # written at different times.
    catalyst_edit_fixtures(dir, "refused = TRUE", "refused = FALSE")
    message <- catalyst_error_message(catalyst_check_export(dir))
    if (is.null(message)) {
      stop("a fixture that disagrees with the domain check was accepted")
    }
    if (!grepl("case", message, fixed = TRUE)) {
      stop(sprintf("the refusal must name the case that disagreed, it said: %s", message))
    }
  })
  invisible(TRUE)
}

test_an_incomplete_export_is_refused <- function() {
  catalyst_with_example_export(function(dir) {
    unlink(file.path(dir, "fixtures.R"))
    message <- catalyst_error_message(catalyst_check_export(dir))
    if (is.null(message)) {
      stop("an export with no fixtures at all was accepted")
    }
    if (!grepl("fixtures.R", message, fixed = TRUE)) {
      stop(sprintf("the refusal must say what is missing, it said: %s", message))
    }
  })
  invisible(TRUE)
}

test_an_export_with_no_case_at_all_is_refused <- function() {
  catalyst_with_example_export(function(dir) {
    writeLines(c(
      "catalyst_tolerance <- list(relative = 1e-9, absolute = 1e-12)",
      "catalyst_fixtures <- list()"
    ), file.path(dir, "fixtures.R"))
    message <- catalyst_error_message(catalyst_check_export(dir))
    if (is.null(message)) {
      stop("an export that checks nothing was reported as checked")
    }
  })
  invisible(TRUE)
}
