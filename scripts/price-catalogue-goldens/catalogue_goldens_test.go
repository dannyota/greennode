package cli

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"danny.vn/vngcloud/compute"
	"danny.vn/vngcloud/internal/core"
	"danny.vn/vngcloud/internal/discovery"
	"danny.vn/vngcloud/internal/endpoints"
	"danny.vn/vngcloud/internal/pricecatalog"
	"danny.vn/vngcloud/internal/pricequote"
	"danny.vn/vngcloud/internal/quoteinput"
	"danny.vn/vngcloud/internal/testutil"
	"danny.vn/vngcloud/internal/transport"
	"danny.vn/vngcloud/network"
	"danny.vn/vngcloud/pricing"
	projectservice "danny.vn/vngcloud/project"
	"danny.vn/vngcloud/volume"
)

type catalogueReply struct {
	Path string `json:"path"`
	Body string `json:"body"`
}

type catalogueOutcome struct {
	Operation string `json:"operation"`
	Status    int    `json:"status"`
	Code      string `json:"code"`
	Message   string `json:"message"`
}

type repeatedCatalogueRow struct {
	Request int    `json:"request"`
	Count   int    `json:"count"`
	Row     string `json:"row"`
}

type catalogueCase struct {
	Name            string                `json:"name"`
	Service         string                `json:"service"`
	Replies         []catalogueReply      `json:"replies"`
	Requests        int                   `json:"requests"`
	Items           json.RawMessage       `json:"items,omitempty"`
	Error           *catalogueOutcome     `json:"error,omitempty"`
	Identity        string                `json:"identity,omitempty"`
	Repeat          *repeatedCatalogueRow `json:"repeat,omitempty"`
	Queries         []string              `json:"queries"`
	Statuses        map[int]int           `json:"statuses,omitempty"`
	Surrogates      int                   `json:"surrogates,omitempty"`
	ImplicitProject bool                  `json:"implicit_project,omitempty"`
	WirePadding     map[int]int           `json:"wire_padding,omitempty"`
	LocalError      string                `json:"local_error,omitempty"`
	NestedArrays    map[int]int           `json:"nested_arrays,omitempty"`
}

func TestPriceCataloguePortGoldens(t *testing.T) {
	const project = "project-1"
	const prefix = "/v1/" + project
	volumeRows := []catalogueReply{
		{prefix + "/zones", `{"data":[{"uuid":"zone-1","name":"Zone","isEnabled":true}]}`},
		{prefix + "/volume_type_zones", `{"volumeTypeZones":[{"id":"parent-1","zone":{"ipRanges":["10.0.0.0/8"]}}]}`},
		{prefix + "/volumes/encryption_types", `[{"key":"aes_256"}]`},
		{prefix + "/parent-1/volume_types", `{"volumeTypes":[{"id":"type-1","name":"SSD","minSize":1,"maxSize":10}]}`},
		{"/v1/price", `{"optimumPrice":100}`},
	}
	computeRows := []catalogueReply{
		{prefix + "/zones", `{"data":[{"uuid":"zone-1","isEnabled":false}]}`},
		{prefix + "/flavor_zones/codes", `[{"key":"cpu","value":"CPU","description":"platform"}]`},
		{prefix + "/flavor_zones/families", `[{"key":"general","value":"General","condition":{"codes":["cpu"]}}]`},
		{prefix + "/flavors/families/general/platforms/cpu", `[{"flavorId":"flavor-1","flavorZoneId":"group-1","name":"Small","cpu":1,"memory":2,"remainingVms":1,"isSoldOut":false}]`},
	}
	networkRows := []catalogueReply{{"/v1/price", `{"optimumPrice":100}`}, {"/v1/price", `{"optimumPrice":100}`}, {"/v1/price", `{"optimumPrice":100}`}}
	imageRows := []catalogueReply{{prefix + "/images/os", `{"images":[{"id":"image-1","imageType":"ubuntu","imageVersion":"22.04","licence":false,"packageLimit":{"diskSize":20}}]}`}}
	regionRows := []catalogueReply{{"/internal/v1/regions", `{"success":true,"datas":[{"regionId":"region-1","regionName":"HCM04"}]}`}}
	storageRows := []catalogueReply{
		{"/internal/v1/billing/project_types", `{"success":true,"datas":[{"id":1,"name":"Gold","status":1,"allowPeriod":[1],"billableResources":[{"purchaseTypeId":2,"projectTypeId":1,"priceKey":"gold"}]}]}`},
		{"/internal/v1/billing/purchase_types", `{"success":true,"datas":[{"id":2,"name":"Normal","title":"Pay monthly","status":1}]}`},
		{"/billing-api/v1/configurations", `{"success":true,"datas":[{"key":"vos_billing_normal_min_quota","value":"1"}]}`},
		{"/billing-api/v1/configurations", `{"success":true,"datas":[{"key":"vos_billing_normal_max_quota","value":"100"}]}`},
		{"/billing-api/v1/configurations", `{"success":true,"datas":[{"key":"max_project_per_user_per_region","value":"4"}]}`},
		{"/billing-api/v1/configurations", `{"success":true,"datas":[{"key":"enable_iam_checkout","value":"true"}]}`},
	}
	groupRows := []catalogueReply{computeRows[0], computeRows[1], {computeRows[2].Path, `[]`},
		{prefix + "/flavor_zones/product", `{"flavorZones":[{"id":"group-1","zoneId":"zone-1"}]}`},
		{prefix + "/flavor_zones/customs", `[]`}, {prefix + "/flavors/customs", `[]`},
		{prefix + "/group-1/flavors", `{"flavors":[]}`},
	}

	var cases []catalogueCase
	add := func(service string, base []catalogueReply, row int, name, body string) {
		replies := append([]catalogueReply(nil), base...)
		replies[row].Body = body
		cases = append(cases, catalogueCase{Name: service + "/" + name, Service: service, Replies: replies})
	}
	for _, entry := range []struct{ name, body string }{
		{"empty", ""}, {"whitespace", " \t\n"}, {"malformed", "{broken"}, {"trailing-value", "{} {}"},
		{"duplicate", `{"data":[],"data":[]}`},
		{"zone-unpaired-surrogate", `{"data":[{"uuid":"zone-1","name":"\uD800","isEnabled":true}]}`},
		{"zone-null-name", `{"data":[{"uuid":"zone-1","name":null,"isEnabled":true}]}`},
		{"zone-wrong-name", `{"data":[{"uuid":"zone-1","name":7,"isEnabled":true}]}`},
		{"zone-underscore", `{"data":[{"uuid":"zone_1","isEnabled":true}]}`},
		{"zone-missing-enabled", `{"data":[{"uuid":"zone-1"}]}`},
		{"zone-null-enabled", `{"data":[{"uuid":"zone-1","isEnabled":null}]}`},
		{"zone-unknown", `{"data":[{"uuid":"zone-1","isEnabled":true,"extra":{"items":[1,null]}}]}`},
	} {
		add("volume", volumeRows, 0, entry.name, entry.body)
	}
	add("volume", volumeRows, 1, "parent-null-array-element", `{"volumeTypeZones":[{"id":"parent-1","zone":{"ipRanges":[null]}}]}`)
	add("volume", volumeRows, 1, "parent-wrong-array-element", `{"volumeTypeZones":[{"id":"parent-1","zone":{"ipRanges":[7]}}]}`)
	add("volume", volumeRows, 1, "parent-wrong-unused-field", `{"volumeTypeZones":[{"id":"parent-1","zone":{"serverCount":false}}]}`)
	add("volume", volumeRows, 1, "parent-underscore", `{"volumeTypeZones":[{"id":"parent_1"}]}`)
	add("volume", volumeRows, 2, "encryption-null-unused", `[{"key":"aes_256","displayKey":null}]`)
	add("volume", volumeRows, 2, "encryption-wrong-unused", `[{"key":"aes_256","displayKey":7}]`)
	add("volume", volumeRows, 3, "type-zero-values", `{"volumeTypes":[{"id":"type-1","volumeTypeId":null,"name":"SSD","iops":-0,"throughPut":null,"minSize":1,"maxSize":10}]}`)
	add("volume", volumeRows, 3, "type-wrong-unused-field", `{"volumeTypes":[{"id":"type-1","name":"SSD","minSize":1,"maxSize":10,"zoneId":7}]}`)
	add("volume", volumeRows, 3, "type-underscore", `{"volumeTypes":[{"id":"type_1","name":"SSD","minSize":1,"maxSize":10}]}`)
	add("volume", volumeRows, 3, "type-missing-name", `{"volumeTypes":[{"id":"type-1","minSize":1,"maxSize":10}]}`)
	add("compute", computeRows, 1, "platform-null-unused", `[{"key":"cpu","value":null,"description":null}]`)
	add("compute", computeRows, 1, "platform-wrong-unused", `[{"key":"cpu","value":7}]`)
	add("compute", computeRows, 1, "platform-case-alias", `[{"key":"cpu","Value":"CPU"}]`)
	add("compute", computeRows, 1, "platform-unknown", `[{"key":"cpu","extra":7}]`)
	add("compute", computeRows, 1, "platform-underscore", `[{"key":"cpu_1"}]`)
	add("compute", computeRows, 2, "family-null-value", `[{"key":"general","value":null,"condition":{"codes":["cpu"]}}]`)
	add("compute", computeRows, 2, "family-wrong-unused", `[{"key":"general","description":7,"condition":{"codes":["cpu"]}}]`)
	add("compute", computeRows, 2, "family-null-codes", `[{"key":"general","condition":{"codes":null}}]`)
	add("compute", computeRows, 2, "family-empty-codes", `[{"key":"general","condition":{"codes":[]}}]`)
	add("compute", computeRows, 2, "family-null-code", `[{"key":"general","condition":{"codes":[null]}}]`)
	flavor := computeRows[3].Body
	add("compute", computeRows, 3, "flavor-underscore", strings.Replace(flavor, "flavor-1", "flavor_1", 1))
	add("compute", computeRows, 3, "flavor-zone-underscore", strings.Replace(flavor, "group-1", "group_1", 1))
	add("compute", computeRows, 3, "flavor-null-required", strings.Replace(flavor, `"cpu":1`, `"cpu":null`, 1))
	add("compute", computeRows, 3, "flavor-null-optional", strings.Replace(flavor, `"cpu":1`, `"cpu":1,"bandwidth":null`, 1))
	add("compute", computeRows, 3, "flavor-metadata-opaque", strings.Replace(flavor, `"cpu":1`, `"cpu":1,"metaData":7`, 1))
	add("compute", computeRows, 3, "flavor-wrong-unused", strings.Replace(flavor, `"cpu":1`, `"cpu":1,"bandwidthUnit":7`, 1))
	for _, entry := range []struct{ name, body string }{
		{"quote-null-currency", `{"optimumPrice":100,"currency":null,"originalPrice":null}`},
		{"quote-wrong-currency", `{"optimumPrice":100,"currency":7}`},
		{"quote-wrong-original", `{"optimumPrice":100,"originalPrice":"100"}`},
		{"quote-opaque-breakdown", `{"optimumPrice":100,"propertiesPrice":"ignored"}`},
		{"quote-missing-required", `{"currency":"VND"}`},
		{"quote-unknown", `{"optimumPrice":100,"extra":7}`},
		{"quote-empty", ""}, {"quote-whitespace", " \n"},
		{"quote-duplicate", `{"optimumPrice":100,"optimumPrice":200}`},
	} {
		add("network", networkRows, 0, entry.name, entry.body)
	}
	add("images", imageRows, 0, "image-valid-evidence", imageRows[0].Body)
	add("images", imageRows, 0, "image-null-evidence", `{"images":[{"id":"image-1","imageType":"ubuntu","imageVersion":"22.04","licence":null,"packageLimit":{"diskSize":null}}]}`)
	add("images", imageRows, 0, "image-opaque-evidence", `{"images":[{"id":"image-1","imageType":"ubuntu","imageVersion":"22.04","licence":7,"packageLimit":{"diskSize":"no"}}]}`)
	add("images", imageRows, 0, "image-wrong-identity", `{"images":[{"id":"image-1","imageType":"ubuntu","imageVersion":7}]}`)
	add("images", imageRows, 0, "image-underscore", `{"images":[{"id":"image_1","imageType":"ubuntu","imageVersion":"22.04"}]}`)
	add("regions", regionRows, 0, "region-unknown", `{"success":true,"datas":[{"regionId":"region-1","regionName":"HCM04","extra":7}]}`)
	add("regions", regionRows, 0, "region-null-name", `{"success":true,"datas":[{"regionId":"region-1","regionName":null}]}`)
	add("regions", regionRows, 0, "region-underscore", `{"success":true,"datas":[{"regionId":"region_1","regionName":"HCM04"}]}`)

	add("compute-non-public", groupRows, 3, "group-null-name", `{"flavorZones":[{"id":"group-1","zoneId":"zone-1","name":null}]}`)
	add("compute-non-public", groupRows, 3, "group-wrong-unused", `{"flavorZones":[{"id":"group-1","zoneId":"zone-1","description":7}]}`)
	add("compute-non-public", groupRows, 3, "group-zone-underscore", `{"flavorZones":[{"id":"group-1","zoneId":"zone_1"}]}`)
	add("compute-non-public", groupRows, 4, "unsupported-custom-row", `[{}]`)
	add("storage", storageRows, 0, "storage-null-leaves", strings.Replace(storageRows[0].Body, `"allowPeriod":[1],"billableResources":[`, `"title":null,"allowPeriod":[null,1],"billableResources":[null,`, 1))
	add("storage", storageRows, 0, "storage-wrong-unused-leaf", strings.Replace(storageRows[0].Body, `"priceKey":"gold"`, `"priceKey":"gold","name":7`, 1))
	add("storage", storageRows, 0, "storage-null-required", strings.Replace(storageRows[0].Body, `"allowPeriod":[1]`, `"allowPeriod":null`, 1))
	add("storage", storageRows, 1, "storage-null-purchase-title", strings.Replace(storageRows[1].Body, `"title":"Pay monthly"`, `"title":null`, 1))
	add("storage", storageRows, 1, "storage-wrong-purchase-title", strings.Replace(storageRows[1].Body, `"title":"Pay monthly"`, `"title":7`, 1))
	add("storage", storageRows, 2, "storage-null-config-value", strings.Replace(storageRows[2].Body, `"value":"1"`, `"value":null`, 1))
	add("storage", storageRows, 2, "storage-wrong-unused-envelope", strings.Replace(storageRows[2].Body, `"success":true`, `"success":true,"errorMsg":7`, 1))

	instanceRows := []catalogueReply{{"/v1/price", `{"propertiesPrice":[{"name":"INSTANCE TYPE","monthlyPrice":100,"optimumPrice":100}]}`}}
	estimateRows := []catalogueReply{{"/v1/price", `{"optimumPrice":100,"propertiesPrice":[{"name":"INSTANCE TYPE","monthlyPrice":100}]}`}}
	add("instance", instanceRows, 0, "instance-null-unselected", `{"propertiesPrice":[null,{"name":null,"monthlyPrice":null},{"name":"INSTANCE TYPE","monthlyPrice":100,"optimumPrice":100}]}`)
	add("instance", instanceRows, 0, "instance-wrong-unselected", `{"propertiesPrice":[{"name":"IGNORED","discountPercent":"10"},{"name":"INSTANCE TYPE","monthlyPrice":100,"optimumPrice":100}]}`)
	add("instance", instanceRows, 0, "instance-null-required", `{"propertiesPrice":[{"name":"INSTANCE TYPE","monthlyPrice":null,"optimumPrice":100}]}`)
	add("estimate", estimateRows, 0, "estimate-null-name", `{"optimumPrice":100,"propertiesPrice":[{"name":null,"monthlyPrice":100}]}`)
	add("estimate", estimateRows, 0, "estimate-null-row", `{"optimumPrice":100,"propertiesPrice":[null]}`)
	add("estimate", estimateRows, 0, "estimate-wrong-unused-root", `{"optimumPrice":100,"originalPrice":false}`)
	add("estimate", estimateRows, 0, "estimate-opaque-line", `{"optimumPrice":100,"propertiesPrice":[{"monthlyPrice":100,"description":7,"currentPrice":"opaque"}]}`)

	add("regions", regionRows, 0, "region-simple-uppercase", `{"success":true,"datas":[{"regionId":"region-1","regionName":"Straße"},{"regionId":"region-2","regionName":"STRASSE"},{"regionId":"region-3","regionName":"ᾀ"},{"regionId":"region-4","regionName":"ἈΙ"}]}`)
	add("regions", regionRows, 0, "region-long-id", `{"success":true,"datas":[{"regionId":"`+strings.Repeat("a", 129)+`","regionName":"HCM04"}]}`)
	add("regions", regionRows, 0, "region-duplicate-simple-uppercase", `{"success":true,"datas":[{"regionId":"region-1","regionName":"ᾀ"},{"regionId":"region-2","regionName":"ᾈ"}]}`)
	add("compute", computeRows, 3, "exact-large-flavor-integers", strings.Replace(flavor, `"cpu":1`, `"cpu":1,"bandwidth":9007199254740993`, 1))
	quotePath := "/billing-api/v2/price"
	storageQuotes := []catalogueReply{{quotePath, `{"success":true,"data":{"optimumPrice":100,"propertiesPrice":[{"name":"Storage","monthlyPrice":40},{"name":"Requests","monthlyPrice":60},{"name":"Unknown","monthlyPrice":null}]}}`}}
	add("estimate-storage", storageQuotes, 0, "storage-quote-success", storageQuotes[0].Body)
	add("estimate-storage", storageQuotes, 0, "storage-quote-refusal-before-pagination", `{"success":false,"code":403,"isNext":true}`)
	add("estimate-storage", storageQuotes, 0, "storage-quote-refusal-typed-is-next", `{"success":false,"code":403,"isNext":"bad"}`)
	add("estimate-storage", storageQuotes, 0, "storage-quote-success-rejects-pagination", `{"success":true,"isNext":true,"data":{"optimumPrice":100}}`)
	add("estimate", estimateRows, 0, "estimate-full-breakdown", `{"optimumPrice":100,"propertiesPrice":[{"name":"First","monthlyPrice":30},{"name":"Second","monthlyPrice":69.5},{"name":"Unknown"}]}`)
	partialQuotes := append(append([]catalogueReply(nil), regionRows...), storageRows...)
	partialQuotes = append(partialQuotes, catalogueReply{"/v1/price", `{"optimumPrice":25,"propertiesPrice":[{"name":"IP","monthlyPrice":25}]}`}, catalogueReply{quotePath, `{"success":false,"code":112}`}, storageQuotes[0])
	add("estimate-partial", partialQuotes, len(partialQuotes)-2, "retains-quotes-and-continues-after-112", partialQuotes[len(partialQuotes)-2].Body)
	flavorRow := strings.TrimSuffix(strings.TrimPrefix(flavor, "["), "]")
	partialFlavors := append([]catalogueReply(nil), computeRows...)
	partialFlavors[2].Body = `[{"key":"general","condition":{"codes":["cpu"]}},{"key":"bad","condition":{"codes":["cpu"]}},{"key":"later","condition":{"codes":["cpu"]}}]`
	partialFlavors = append(partialFlavors, catalogueReply{prefix + "/flavors/families/bad/platforms/cpu", `{"flavorId":"bad-row"}`}, catalogueReply{prefix + "/flavors/families/later/platforms/cpu", strings.Replace(flavor, "flavor-1", "flavor-2", 1)})
	add("compute-partial", partialFlavors, 4, "late-invalid-row-does-not-consume-budget", partialFlavors[4].Body)
	cases[len(cases)-1].Repeat = &repeatedCatalogueRow{Request: 4, Count: 9_999, Row: flavorRow}

	add("volume", volumeRows, 0, "malformed-after-first-value", `{} {broken`)
	for _, status := range []int{403, 503} {
		name := fmt.Sprintf("malformed-after-first-value-%d", status)
		add("volume", volumeRows, 0, name, `{} {broken`)
		cases[len(cases)-1].Statuses = map[int]int{0: status}
	}
	add("volume", volumeRows, 0, "valid-stream-forbidden", `{} {}`)
	cases[len(cases)-1].Statuses = map[int]int{0: 403}
	add("volume", append([]catalogueReply{volumeRows[0]}, volumeRows...), 0, "valid-stream-retry", `{} {}`)
	cases[len(cases)-1].Statuses = map[int]int{0: 503}

	add("volume", volumeRows, 0, "adjacent-scalar-stream", `1true`)
	add("volume", volumeRows, 0, "adjacent-scalar-stream-forbidden", `1true`)
	cases[len(cases)-1].Statuses = map[int]int{0: 403}

	secondParent := catalogueReply{prefix + "/parent-2/volume_types", `{"volumeTypes":[]}`}
	padded := append(append([]catalogueReply(nil), volumeRows[:4]...), secondParent, volumeRows[4])
	padded[1].Body = `{"volumeTypeZones":[{"id":"parent-1"},{"id":"parent-2"}]}`
	for i := range padded[:5] {
		if i == 2 {
			padded[i].Body = `[{"key":"aes_256","unknown":"@surrogates@"}]`
		} else {
			padded[i].Body = strings.TrimSuffix(padded[i].Body, "}") + `,"unknown":"@surrogates@"}`
		}
	}
	padded[0].Body = " \n" + padded[0].Body + "\t "
	add("volume", padded, 4, "raw-escaped-pages-exceed-budget", padded[4].Body)
	cases[len(cases)-1].Surrogates = (7 << 20) / 6
	recovered := append(append([]catalogueReply(nil), padded[:5]...), secondParent, volumeRows[4])
	add("volume", recovered, 4, "retry-body-does-not-consume-budget", padded[4].Body)
	cases[len(cases)-1].Surrogates = (7 << 20) / 6
	cases[len(cases)-1].Statuses = map[int]int{4: 503}

	add("volume", volumeRows, 0, "adjacent-zero-stream", `01`)
	add("volume", append([]catalogueReply{volumeRows[0]}, volumeRows...), 0, "adjacent-zero-stream-retry", `01`)
	cases[len(cases)-1].Statuses = map[int]int{0: 503}
	add("volume", padded, 4, "raw-escaped-pages-exceed-budget-before-duplicate", strings.Replace(padded[4].Body, `"volumeTypes":[]`, `"volumeTypes":[],"volumeTypes":[]`, 1))
	cases[len(cases)-1].Surrogates = (7 << 20) / 6
	projectPage := catalogueReply{"/v1/projects", `{"projects":[{"projectId":"project-1","region":"hcm-3"}]}`}
	for _, row := range []struct {
		name   string
		status int
		body   string
	}{
		{"implicit-project-retry", 503, `{} {}`},
		{"implicit-project-malformed", 503, `{} {broken`},
		{"implicit-project-multiple", 200, `{} {}`},
		{"implicit-project-adjacent-zero", 403, `01`},
		{"implicit-project-whitespace", 200, " \n\t"},
		{"implicit-project-empty", 200, ""},
	} {
		replies := append([]catalogueReply{projectPage, projectPage}, volumeRows...)
		add("volume", replies, 0, row.name, row.body)
		cases[len(cases)-1].ImplicitProject = true
		cases[len(cases)-1].Statuses = map[int]int{0: row.status}
	}
	large := append(append([]catalogueReply{projectPage}, volumeRows[:4]...), secondParent, volumeRows[4])
	large[2].Body = `{"volumeTypeZones":[{"id":"parent-1"},{"id":"parent-2"}]}`
	sizes := map[int]int{}
	for i := range large[:6] {
		if i == 3 {
			large[i].Body = `[{"key":"aes_256","unknown":"@padding@"}]`
		} else {
			large[i].Body = strings.TrimSuffix(large[i].Body, "}") + `,"unknown":"@padding@"}`
		}
		sizes[i] = 6 << 20
	}
	sizes[5] = (2 << 20) - len(large[6].Body)
	large[0].Body = " \n" + large[0].Body + "\t "
	add("volume", large, 0, "implicit-project-whitespace-excluded-from-budget", large[0].Body)
	cases[len(cases)-1].ImplicitProject = true
	cases[len(cases)-1].WirePadding = sizes

	partialVolume := append(append([]catalogueReply(nil), volumeRows[:4]...), secondParent, volumeRows[4])
	partialVolume[1].Body = `{"volumeTypeZones":[{"id":"parent-1"},{"id":"parent-2"}]}`
	add("volume-partial", partialVolume, 4, "partial-volume-keeps-accepted-type", `{}`)
	cases[len(cases)-1].Statuses = map[int]int{4: 403}
	cases = append(cases, catalogueCase{Name: "estimate/limit-prefix", Service: "estimate-limit", Replies: []catalogueReply{}, Queries: []string{}})

	add("volume", append([]catalogueReply{volumeRows[0]}, volumeRows...), 0, "deep-array-stream-rejected-before-retry", `0`)
	cases[len(cases)-1].Statuses = map[int]int{0: 503}
	cases[len(cases)-1].NestedArrays = map[int]int{0: 10001}
	add("volume", volumeRows, 0, "deep-array-success-exceeds-document-depth", `0`)
	cases[len(cases)-1].NestedArrays = map[int]int{0: 10001}
	add("volume", volumeRows, 0, "deep-array-within-document-depth", `0`)
	cases[len(cases)-1].NestedArrays = map[int]int{0: 10000}

	projectRows := []catalogueReply{{"/v1/projects", `{"projects":[{"projectId":"project-1","name":"Project","region":"hcm-3","userId":-0}]}`}}
	add("project", projectRows, 0, "negative-zero-user-id", projectRows[0].Body)
	add("project", projectRows, 0, "plain-text-created", `retired response`)
	cases[len(cases)-1].Statuses = map[int]int{0: 201}
	add("project", append([]catalogueReply{projectRows[0]}, projectRows...), 0, "plain-text-retry", `retry later`)
	cases[len(cases)-1].Statuses = map[int]int{0: 503}
	add("project", projectRows, 0, "two-complete-values", `{} {}`)
	add("project", projectRows, 0, "malformed-trailing-value", `{} {broken`)

	for i := range cases {
		c := &cases[i]
		t.Run(c.Name, func(t *testing.T) {
			var lock sync.Mutex
			fixture := testutil.NewConfig
			if len(c.Statuses) > 0 {
				fixture = testutil.NewRetryConfig
			}
			handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				lock.Lock()
				defer lock.Unlock()
				n := c.Requests
				c.Requests++
				if n >= len(c.Replies) || r.URL.Path != c.Replies[n].Path {
					t.Errorf("unexpected request %d: %s", n, r.URL.Path)
					w.WriteHeader(http.StatusNotFound)
					return
				}
				c.Queries = append(c.Queries, r.URL.RawQuery)
				body := c.Replies[n].Body
				if c.Surrogates > 0 && strings.Contains(body, "@surrogates@") {
					body = strings.Replace(body, "@surrogates@", strings.Repeat(`\uD800`, c.Surrogates), 1)
				}
				if size := c.WirePadding[n]; size > 0 {
					extra := size - len(strings.Trim(body, " \n\r\t")) + len("@padding@")
					body = strings.Replace(body, "@padding@", strings.Repeat("x", extra), 1)
				}
				if depth := c.NestedArrays[n]; depth > 0 {
					body = strings.Repeat("[", depth) + body + strings.Repeat("]", depth)
				}
				if status := c.Statuses[n]; status != 0 {
					w.WriteHeader(status)
				}
				if r := c.Repeat; r != nil && r.Request == n {
					body = "[" + strings.Repeat(r.Row+",", r.Count) + body + "]"
				}
				_, _ = w.Write([]byte(body))
			})
			var cfg core.Config
			if c.ImplicitProject {
				peer := httptest.NewServer(handler)
				defer peer.Close()
				cfg = core.NewTestConfig("hcm-3", "", endpoints.Set{Region: "hcm-3", VServer: peer.URL + "/", Portal: peer.URL + "/"}, transport.New(transport.Config{HTTPClient: peer.Client(), RetryCount: 3, RetryInterval: time.Millisecond}))
			} else {
				cfg = fixture(t, handler)
			}
			ctx := computeDiscoveryCLIContext()
			var items any
			var err error
			switch c.Service {
			case "volume", "volume-partial":
				out, e := volume.New(cfg).ListVolumePrices(ctx, &volume.ListVolumePricesInput{Encryption: "aes_256", AllowPartial: c.Service == "volume-partial"})
				err = e
				if out != nil {
					items = out.Items
					if c.Service == "volume-partial" {
						items = []any{out.Items, out.Metadata.Failures}
					}
				}
			case "estimate-limit":
				_, err = pricing.New(cfg).Estimate(ctx, &pricing.EstimateInput{MaxRequests: 1, PublicIPs: []pricing.EstimatePublicIP{
					{Key: "first", Kind: "public-vip", Type: "public", Quantity: 1},
					{Key: "second", Kind: "public-vip", Type: "public-vm", Quantity: 1},
				}})
			case "compute", "compute-non-public", "compute-partial":
				out, e := compute.New(cfg).ListFlavorPrices(ctx, &compute.ListFlavorPricesInput{IncludeNonPublic: c.Service == "compute-non-public", AllowPartial: c.Service == "compute-partial"})
				err = e
				if out != nil {
					items = out.Items
					if c.Service == "compute-partial" {
						items = []any{out.Items, out.Metadata.Failures}
					}
				}
			case "estimate-partial":
				input := &pricing.EstimateInput{
					PublicIPs:    []pricing.EstimatePublicIP{{Key: "first", Kind: "public-vip", Type: "public", Quantity: 1}},
					Storage:      []pricing.EstimateStorage{{Key: "refused", Type: "Gold", Quantity: 1, QuotaGB: 1}, {Key: "later", Type: "Gold", Quantity: 1, QuotaGB: 2}},
					AllowPartial: true,
				}
				out, e := pricing.New(cfg).Estimate(ctx, input)
				err = e
				if out != nil {
					items = []any{out.Items, out.Metadata.Failures, out.Total}
				}
			case "project":
				out, e := projectservice.New(cfg).ListProjects(ctx, nil)
				err = e
				if out != nil {
					rows := make([]map[string]any, len(out.Items))
					for i, row := range out.Items {
						rows[i] = map[string]any{"ID": row.ID, "Name": row.Name, "Region": row.Region, "Status": row.Status, "IsDefault": row.IsDefault, "UserID": row.UserID}
					}
					items = rows
				}
			case "network":
				out, e := network.New(cfg).ListNetworkPrices(ctx, nil)
				err = e
				if out != nil {
					items = out.Items
				}

			case "instance", "estimate", "estimate-storage":
				s, e := discovery.New(ctx, 10)
				if e != nil {
					t.Fatal(e)
				}
				defer s.Close()
				client := core.ClientOf(cfg)
				req := pricecatalog.RegionalQuote(client, "pricing.GetQuote", quoteinput.PublicIP("public"))
				if c.Service == "estimate-storage" {
					req := pricecatalog.StorageQuote(client, "pricing.GetQuote", "region-1", quoteinput.Storage(1, 2, 1))
					v, e := pricequote.Quote(client, s, req, true)
					err = e
					items = []any{v.Price, v.BreakdownMismatch, v.Breakdown}
					break
				}
				_, err = s.DoJSON(client, req, nil, func(status int, raw json.RawMessage) error {
					if c.Service == "instance" {
						v, e := pricequote.DecodeInstance(req.Operation, status, raw)
						items = v
						return e
					}
					v, e := pricequote.DecodeEstimate(req.Operation, status, raw, false)
					items = []any{v.Price, v.BreakdownMismatch, v.Breakdown}
					return e
				})
			case "images", "regions", "storage":
				s, e := discovery.New(ctx, 10)
				if e != nil {
					t.Fatal(e)
				}
				defer s.Close()

				coreClient := core.ClientOf(cfg)
				switch c.Service {
				case "images":
					rows, e := pricecatalog.Images(coreClient, s, project, "zone-1")
					err = e
					if e == nil {
						image := rows[0]
						items = []any{image.ID, image.ImageType, image.ImageVersion, image.UnlicensedUbuntu(), image.PackageLimit.DiskSize}
					}
				case "regions":
					rows, e := pricecatalog.StorageRegions(coreClient, s)
					err = e
					if e == nil {
						items = rows
					}
				case "storage":
					out, e := pricecatalog.ReadStorage(coreClient, s, "pricing.Estimate", "region-1")
					err = e
					if e == nil {
						items = []any{out.MinQuota, out.MaxQuota, out.MaxProjects, out.IAMCheckout, out.Types[0].AllowPeriod, len(out.Types[0].BillableResources)}
					}
				}
			}

			if err != nil {
				var api *core.APIError
				if errors.As(err, &api) {
					c.Error = &catalogueOutcome{api.Operation, api.StatusCode, api.Code, api.Message}
				} else if !errors.Is(err, discovery.ErrLimit) {
					t.Fatalf("expected API or discovery error: %v", err)
				}
				c.Replies = c.Replies[:c.Requests]
				switch {
				case errors.Is(err, discovery.ErrLimit):
					c.Identity = "EstimateLimit"
					c.LocalError = err.Error()
				case errors.Is(err, pricing.ErrDiscoveryIncomplete):
					c.Identity = "DiscoveryIncomplete"
				case errors.Is(err, core.ErrPermission):
					c.Identity = "PermissionDenied"
				}
				if c.Service != "estimate-partial" && c.Service != "compute-partial" && c.Service != "volume-partial" {
					items = nil
				}
			}
			if items != nil {
				c.Items, err = json.Marshal(items)
				if err != nil {
					t.Fatal(err)
				}
			}
		})
	}
	file, err := os.Create(os.Getenv("PRICE_CATALOGUE_GOLDEN"))
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	if _, err := file.WriteString("[\n"); err != nil {
		t.Fatal(err)
	}
	for i, c := range cases {
		if i != 0 {
			if _, err := file.WriteString(",\n"); err != nil {
				t.Fatal(err)
			}
		}
		row, err := json.Marshal(c)
		if err != nil {
			t.Fatal(err)
		}
		if _, err := file.Write(row); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := file.WriteString("\n]\n"); err != nil {
		t.Fatal(err)
	}
}
