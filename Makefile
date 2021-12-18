# Repositories for integration tests that will be cloned inside `integration-tests/testdata/REPO` folders
INTEGRATION_TESTS_REPOS = \
	mds1/drai \
	reflexer-labs/geb \
	hexonaut/guni-lev \
	Rari-Capital/solmate \
	Arachnid/solidity-stringutils \
	rari-capital/vaults \
	makerdao/multicall \
	gakonst/lootloose

integration-tests-testdata: $(INTEGRATION_TESTS_REPOS)

$(INTEGRATION_TESTS_REPOS):
	@FOLDER=$(shell dirname "$0")/integration-tests/testdata/$(lastword $(subst /, ,$@));\
	if [ ! -d $$FOLDER/.git ] ; then git clone --depth 1 --recursive https://github.com/$@ $$FOLDER;\
	else cd $$FOLDER; git pull --recurse-submodules; fi

fmt-testdata:
	@FOLDER=$(shell dirname "$0")/fmt/testdata;\
	if [ ! -d $$FOLDER/.git ] ; then git clone --depth 1 --recursive https://github.com/prettier-solidity/prettier-plugin-solidity $$FOLDER/prettier-plugin-solidity;\
	else cd $$FOLDER; git pull --recurse-submodules; fi

testdata: integration-tests-testdata fmt-testdata
