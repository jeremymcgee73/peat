.PHONY: pdf clean check-deps

SOURCE = peat-protocol-concept-draft.md
OUTPUT_DIR = build
PDF = $(OUTPUT_DIR)/Peat_Protocol_Concept_Draft.pdf

PANDOC_OPTIONS = \
	--from markdown \
	--standalone \
	--table-of-contents \
	--toc-depth=3 \
	--pdf-engine=xelatex

pdf: $(PDF)

$(PDF): $(SOURCE) | $(OUTPUT_DIR)
	pandoc $(PANDOC_OPTIONS) -o $@ $<

$(OUTPUT_DIR):
	mkdir -p $@

check-deps:
	@command -v pandoc >/dev/null || (printf '%s\n' "ERROR: pandoc is required" && exit 1)
	@command -v xelatex >/dev/null || (printf '%s\n' "ERROR: xelatex is required" && exit 1)

clean:
	rm -rf $(OUTPUT_DIR)
